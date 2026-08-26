//! The shell evaluator: walks the AST and executes it.
//!
//! M2 replaces the M0 argv-splitter with the full parser and adds control
//! flow, expansions, redirections, pipelines, builtins and functions.

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use cake_blacklist::CommandBlacklist;
use cake_env::{EnvVar, EnvStack};
use cake_platform::{ChildFd, Fd, ProcessHandle, SpawnConfig, WaitOptions, WaitStatus};
use cake_proc::ProcStatus;
use cake_syntax::{
    parse, AndOrList, AndOrOp, AssignmentValue, CaseCommand, Command, CommandKind, CompleteCommand,
    ForCommand, List, Pipeline, Program, Separator, SimpleCommand, WhileCommand,
};

use crate::arith::eval_arith;
use crate::builtins;
use crate::cond::eval_cond;
use crate::expand::{expand_word, expand_word_quoted, ExpandCtx};
use crate::glob::glob_match;
use crate::redirect::{setup_redirects, CommandFds};
use crate::resolve::{resolve_command, CommandSpec};

/// The outcome of evaluating a command string.
#[derive(Debug)]
pub struct EvalOutcome {
    pub status: ProcStatus,
    /// A message for the shell driver to print to stderr, if any.
    pub error: Option<String>,
}

/// The result of evaluating one command: either a completed status or an
/// external process that still needs to be waited on.
enum EvalResult {
    Done(ProcStatus),
    Spawned(ProcessHandle),
}

/// The shell evaluator.
#[derive(Debug)]
pub struct Executor {
    pub env: EnvStack,
    pub last_status: ProcStatus,
    /// Positional parameters; `positional[0]` is `$0`.
    pub positional: Vec<String>,
    pub functions: BTreeMap<String, Command>,
    /// Set when `exit` is called; checked by the outer eval loop.
    exit_requested: Option<i32>,
    /// The shell's own pid (`$$`). Set by the driver.
    pub shell_pid: i32,
    /// Pids of background jobs not yet reaped.
    background: Vec<ProcessHandle>,
    /// Commands that were not found (persisted by the driver).
    pub blacklist: CommandBlacklist,
}

impl Executor {
    pub fn new(env: EnvStack) -> Self {
        Self {
            env,
            last_status: ProcStatus::NotStarted,
            positional: alloc::vec!["cake".into()],
            functions: BTreeMap::new(),
            exit_requested: None,
            shell_pid: 0,
            background: Vec::new(),
            blacklist: CommandBlacklist::new(),
        }
    }

    pub fn request_exit(&mut self, code: i32) {
        self.exit_requested = Some(code);
    }

    /// Whether the user ran `exit` (and with which code). The interactive
    /// reader checks this after each evaluation to leave the loop.
    pub fn exit_requested(&self) -> Option<i32> {
        self.exit_requested
    }

    /// Clear a pending `exit` request, returning the code.
    pub fn take_exit_requested(&mut self) -> Option<i32> {
        self.exit_requested.take()
    }

    /// Reap any exited background jobs, removing them from the list.
    pub fn reap_background(&mut self) {
        self.background.retain(|h| {
            matches!(
                cake_platform::get().wait(h, WaitOptions::NOHANG),
                Ok(WaitStatus::Stopped(_)) | Err(_)
            )
        });
    }

    // --- entry point -----------------------------------------------------

    /// Evaluate a command string (used by `cake -c` and the interactive
    /// reader). Leaves any `exit` request intact so the caller can observe it
    /// via [`Self::exit_requested`].
    pub fn eval_str(&mut self, src: &str) -> EvalOutcome {
        match parse(src) {
            Ok(prog) => {
                let status = self.eval_program(&prog);
                self.last_status = status;
                let final_status = self.exit_requested().map(ProcStatus::Exit).unwrap_or(status);
                EvalOutcome {
                    status: final_status,
                    error: None,
                }
            }
            Err(errs) => {
                let msg = if errs.is_empty() {
                    "cake: parse error".into()
                } else {
                    alloc::format!("cake: {}", errs[0].message)
                };
                let status = ProcStatus::Exit(2);
                self.last_status = status;
                EvalOutcome {
                    status,
                    error: Some(msg),
                }
            }
        }
    }

    // --- program / list walking -----------------------------------------

    fn eval_program(&mut self, prog: &Program) -> ProcStatus {
        let mut status = ProcStatus::Exit(0);
        for cc in &prog.commands {
            if self.exit_requested.is_some() {
                break;
            }
            status = self.eval_complete(cc);
        }
        status
    }

    fn eval_complete(&mut self, cc: &CompleteCommand) -> ProcStatus {
        match cc.separator {
            Separator::Amp => self.eval_and_or_background(&cc.list),
            _ => self.eval_and_or(&cc.list),
        }
    }

    fn eval_list(&mut self, list: &List) -> ProcStatus {
        let mut status = ProcStatus::Exit(0);
        for item in &list.items {
            if self.exit_requested.is_some() {
                break;
            }
            status = self.eval_and_or(item);
        }
        status
    }

    fn eval_and_or(&mut self, aol: &AndOrList) -> ProcStatus {
        let mut status = self.eval_pipeline(&aol.first);
        for (op, pipeline) in &aol.rest {
            if self.exit_requested.is_some() {
                break;
            }
            let need_run = match op {
                AndOrOp::AndAnd => status.success(),
                AndOrOp::OrOr => !status.success(),
            };
            if need_run {
                status = self.eval_pipeline(pipeline);
            }
        }
        self.last_status = status;
        status
    }

    /// `list &` — run asynchronously.
    fn eval_and_or_background(&mut self, aol: &AndOrList) -> ProcStatus {
        let list = aol.clone();
        let exec = &mut *self as *mut Executor;
        let result = cake_platform::get().run_in_child(&mut move || {
            let exec = unsafe { &mut *exec };
            exec.eval_and_or(&list).status_code()
        });
        result.ok();
        ProcStatus::Exit(0)
    }

    // --- pipelines -------------------------------------------------------

    fn eval_pipeline(&mut self, pipeline: &Pipeline) -> ProcStatus {
        let n = pipeline.commands.len();
        if n == 0 {
            return ProcStatus::Exit(0);
        }

        let mut handles: Vec<ProcessHandle> = Vec::new();
        let mut prev_read: Option<Fd> = None;
        let mut last_status = ProcStatus::Exit(1);

        for (i, cmd) in pipeline.commands.iter().enumerate() {
            let is_last = i == n - 1;

            let mut fds = match self.setup_cmd_fds(cmd) {
                Ok(f) => f,
                Err(e) => {
                    self.report_error(&e);
                    return ProcStatus::Exit(1);
                }
            };

            // The read end produced by the previous element feeds this one.
            let old_prev = prev_read;
            if let Some(pr) = old_prev {
                if fds.stdin == ChildFd::Inherit {
                    fds.stdin = ChildFd::Fd(pr);
                }
            }

            // Create the pipe this element writes into (if not last).
            let next_pipe: Option<(Fd, Fd)> = if is_last {
                None
            } else {
                match cake_platform::get().pipe(false) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        self.report_error(&alloc::format!("cake: pipe: {e}"));
                        return ProcStatus::Exit(1);
                    }
                }
            };
            if let Some((r, w)) = next_pipe {
                if fds.stdout == ChildFd::Inherit {
                    fds.stdout = ChildFd::Fd(w);
                }
                prev_read = Some(r);
            }

            match self.eval_command(cmd, &mut fds) {
                Ok(EvalResult::Done(st)) => {
                    last_status = st;
                }
                Ok(EvalResult::Spawned(h)) => {
                    // A spawned process: wait for its final status later. If
                    // this is the last element, its status is the pipeline's.
                    if is_last {
                        last_status = self.wait_for(&h);
                    } else {
                        handles.push(h);
                    }
                }
                Err(e) => {
                    self.report_error(&e);
                    last_status = ProcStatus::Exit(127);
                }
            }

            // The child holds dup2 copies; close the parent's copies now.
            for fd in &fds.owned {
                let _ = cake_platform::get().close(*fd);
            }
            if let Some((_, w)) = next_pipe {
                let _ = cake_platform::get().close(w);
            }
            // The old read end is now owned by this element's child.
            if let Some(pr) = old_prev {
                let _ = cake_platform::get().close(pr);
            }
        }

        for h in &handles {
            let _ = self.wait_for(h);
        }

        if pipeline.negated {
            last_status = ProcStatus::Exit(if last_status.success() { 1 } else { 0 });
        }
        last_status
    }

    fn setup_cmd_fds(&mut self, cmd: &Command) -> Result<CommandFds, String> {
        let ctx = self.ctx();
        setup_redirects(&ctx, &cmd.redirects)
    }

    // --- commands --------------------------------------------------------

    fn eval_command(&mut self, cmd: &Command, fds: &mut CommandFds) -> Result<EvalResult, String> {
        match &cmd.kind {
            CommandKind::Simple(sc) => self.eval_simple(sc, fds),
            CommandKind::Empty => Ok(EvalResult::Done(ProcStatus::Exit(0))),
            CommandKind::If(ifc) => {
                let st = self.apply_fds_in_parent(fds, |exec| exec.eval_if(ifc));
                Ok(EvalResult::Done(st))
            }
            CommandKind::While(w) => {
                let st = self.apply_fds_in_parent(fds, |exec| exec.eval_while(w, false));
                Ok(EvalResult::Done(st))
            }
            CommandKind::Until(w) => {
                let st = self.apply_fds_in_parent(fds, |exec| exec.eval_while(w, true));
                Ok(EvalResult::Done(st))
            }
            CommandKind::For(fc) => {
                let st = self.apply_fds_in_parent(fds, |exec| exec.eval_for(fc));
                Ok(EvalResult::Done(st))
            }
            CommandKind::Case(cs) => {
                let st = self.apply_fds_in_parent(fds, |exec| exec.eval_case(cs));
                Ok(EvalResult::Done(st))
            }
            CommandKind::Block(blk) => {
                let st = self.apply_fds_in_parent(fds, |exec| {
                    exec.env.push_scope();
                    let s = exec.eval_list(&blk.body);
                    exec.env.pop_scope();
                    s
                });
                Ok(EvalResult::Done(st))
            }
            CommandKind::Subshell(sub) => {
                let body = sub.body.clone();
                let fds = fds.clone();
                let exec = &mut *self;
                let handle = cake_platform::get().run_in_child(&mut move || {
                    exec.apply_fds_in_parent(&fds, |e| e.eval_list(&body))
                        .status_code()
                });
                match handle {
                    Ok(h) => Ok(EvalResult::Spawned(h)),
                    Err(e) => Err(alloc::format!("cake: subshell: {e}")),
                }
            }
            CommandKind::Function(fn_cmd) => {
                self.functions.insert(fn_cmd.name.clone(), (*fn_cmd.body).clone());
                Ok(EvalResult::Done(ProcStatus::Exit(0)))
            }
            CommandKind::Arith(ar) => {
                let st = self.apply_fds_in_parent(fds, |exec| eval_arith(exec, &ar.text));
                Ok(EvalResult::Done(st))
            }
            CommandKind::Cond(cond) => {
                let st = self.apply_fds_in_parent(fds, |exec| eval_cond(exec, &cond.text));
                Ok(EvalResult::Done(st))
            }
        }
    }

    fn eval_simple(&mut self, sc: &SimpleCommand, fds: &mut CommandFds) -> Result<EvalResult, String> {
        // Prefix assignments.
        let mut temp_assigns: Vec<(String, String)> = Vec::new();
        let mut shell_assigns: Vec<(String, String)> = Vec::new();
        for a in &sc.assignments {
            let v = match &a.value {
                AssignmentValue::Word(w) => {
                    let ctx = self.ctx();
                    expand_word_quoted(&ctx, w)?
                }
                _ => String::new(),
            };
            if sc.words.is_empty() {
                shell_assigns.push((a.name.clone(), v));
            } else {
                temp_assigns.push((a.name.clone(), v));
            }
        }

        // Expand the command words.
        let mut argv: Vec<String> = Vec::new();
        {
            let ctx = self.ctx();
            for w in &sc.words {
                let fields = expand_word(&ctx, w)?;
                argv.extend(fields);
            }
        }

        if argv.is_empty() {
            for (name, value) in shell_assigns {
                self.env
                    .set(&name, EnvVar::new(value))
                    .map_err(|e| alloc::format!("cake: {e}"))?;
            }
            return Ok(EvalResult::Done(ProcStatus::Exit(0)));
        }

        let cmd_name = argv[0].clone();
        match resolve_command(self, &cmd_name) {
            CommandSpec::Builtin => {
                let argv_c = argv.clone();
                let st = self.apply_fds_in_parent(fds, |exec| {
                    match builtins::run_builtin(exec, &cmd_name, &argv_c) {
                        Some(Ok(st)) => st,
                        Some(Err(e)) => {
                            exec.report_error(&e);
                            ProcStatus::Exit(1)
                        }
                        None => ProcStatus::Exit(127),
                    }
                });
                Ok(EvalResult::Done(st))
            }
            CommandSpec::Function => self.call_function(&cmd_name, &argv, temp_assigns),
            CommandSpec::External(path) => self.run_external(&path, &argv, temp_assigns, fds),
            CommandSpec::NotFound => {
                self.blacklist.insert(&cmd_name);
                self.report_error(&alloc::format!("cake: {cmd_name}: command not found"));
                Ok(EvalResult::Done(ProcStatus::Exit(127)))
            }
        }
    }

    fn call_function(
        &mut self,
        name: &str,
        argv: &[String],
        temp_assigns: Vec<(String, String)>,
    ) -> Result<EvalResult, String> {
        let body = match self.functions.get(name) {
            Some(b) => b.clone(),
            None => return Ok(EvalResult::Done(ProcStatus::Exit(127))),
        };

        let saved_positional = core::mem::replace(
            &mut self.positional,
            argv.iter().cloned().collect(),
        );
        self.env.push_scope();
        for (n, v) in &temp_assigns {
            let _ = self.env.set(n, EnvVar::new(v.clone()));
        }
        let mut fds = CommandFds::default();
        let status = self.eval_command(&body, &mut fds).map(|r| match r {
            EvalResult::Done(st) => st,
            EvalResult::Spawned(h) => self.wait_for(&h),
        });
        self.env.pop_scope();
        self.positional = saved_positional;
        status.map(EvalResult::Done)
    }

    fn run_external(
        &mut self,
        path: &str,
        argv: &[String],
        temp_assigns: Vec<(String, String)>,
        fds: &CommandFds,
    ) -> Result<EvalResult, String> {
        let mut env = self.env.exported_env();
        for (k, v) in &temp_assigns {
            env.push((k.clone(), v.clone()));
        }
        let cfg = SpawnConfig {
            path: path.to_owned(),
            argv: argv.to_vec(),
            env,
            stdin: fds.stdin.clone(),
            stdout: fds.stdout.clone(),
            stderr: fds.stderr.clone(),
            pgroup: None,
            background: false,
        };
        let handle = cake_platform::get()
            .spawn(&cfg)
            .map_err(|e| alloc::format!("cake: {}: {e}", argv[0]))?;
        // The child holds dup2 copies; the parent can close the originals.
        for fd in &fds.owned {
            let _ = cake_platform::get().close(*fd);
        }
        Ok(EvalResult::Spawned(handle))
    }

    fn wait_for(&mut self, handle: &ProcessHandle) -> ProcStatus {
        match cake_platform::get().wait(handle, WaitOptions::NONE) {
            Ok(WaitStatus::Exited(code)) => ProcStatus::Exit(code as i32),
            Ok(WaitStatus::Signaled(sig)) => ProcStatus::Signal(sig.number()),
            Ok(_) => ProcStatus::Exit(1),
            Err(_) => ProcStatus::Exit(127),
        }
    }

    // --- control flow ----------------------------------------------------

    fn eval_if(&mut self, ifc: &cake_syntax::IfCommand) -> ProcStatus {
        for clause in &ifc.clauses {
            let st = self.eval_and_or(&clause.cond);
            if self.exit_requested.is_some() {
                return st;
            }
            if st.success() {
                return self.eval_list(&clause.body);
            }
        }
        match &ifc.else_body {
            Some(b) => self.eval_list(b),
            None => ProcStatus::Exit(0),
        }
    }

    fn eval_while(&mut self, w: &WhileCommand, until: bool) -> ProcStatus {
        loop {
            let st = self.eval_and_or(&w.cond);
            if self.exit_requested.is_some() {
                return st;
            }
            let do_body = if until { !st.success() } else { st.success() };
            if !do_body {
                break;
            }
            self.eval_list(&w.body);
        }
        ProcStatus::Exit(0)
    }

    fn eval_for(&mut self, fc: &ForCommand) -> ProcStatus {
        let words: Vec<String> = match &fc.in_words {
            Some(in_words) => {
                let mut out = Vec::new();
                let ctx = self.ctx();
                for w in in_words {
                    match expand_word(&ctx, w) {
                        Ok(fields) => out.extend(fields),
                        Err(_) => {}
                    }
                }
                out
            }
            None => {
                // `for var; do` == `for var in "$@"`
                if self.positional.len() > 1 {
                    self.positional[1..].to_vec()
                } else {
                    Vec::new()
                }
            }
        };
        for w in words {
            let _ = self
                .env
                .set(&fc.var, EnvVar::new(w));
            self.eval_list(&fc.body);
            if self.exit_requested.is_some() {
                break;
            }
        }
        ProcStatus::Exit(0)
    }

    fn eval_case(&mut self, cs: &CaseCommand) -> ProcStatus {
        let word = {
            let ctx = self.ctx();
            expand_word(&ctx, &cs.word).unwrap_or_default()
        };
        let word_str = word.into_iter().next().unwrap_or_default();
        for arm in &cs.arms {
            for pat in &arm.patterns {
                let pat_str = {
                    let ctx = self.ctx();
                    expand_word(&ctx, pat).unwrap_or_default()
                };
                let pat_str = pat_str.into_iter().next().unwrap_or_default();
                if glob_match(&pat_str, &word_str) {
                    return self.eval_list(&arm.body);
                }
            }
        }
        ProcStatus::Exit(0)
    }

    // --- helpers ---------------------------------------------------------

    fn ctx(&self) -> ExpandCtx<'_> {
        ExpandCtx {
            env: &self.env,
            last_status: self.last_status,
            positional: &self.positional,
            shell_pid: self.shell_pid,
        }
    }

    pub(crate) fn report_error(&mut self, msg: &str) {
        let _ = cake_platform::get().write(2, msg.as_bytes());
        let _ = cake_platform::get().write(2, b"\n");
    }

    /// Apply `fds` in the current process (for builtins/compounds), run `f`,
    /// then restore.
    fn apply_fds_in_parent<T>(&mut self, fds: &CommandFds, f: impl FnOnce(&mut Self) -> T) -> T {
        let p = cake_platform::get();
        let mut saved: Vec<(Fd, Fd)> = Vec::new();
        for (slot, cfg) in [(0i32, &fds.stdin), (1, &fds.stdout), (2, &fds.stderr)] {
            match cfg {
                ChildFd::Fd(fd) => {
                    if let Ok(s) = p.dup(slot) {
                        saved.push((slot, s));
                    }
                    let _ = p.dup2(*fd, slot);
                }
                ChildFd::Close => {
                    if let Ok(s) = p.dup(slot) {
                        saved.push((slot, s));
                    }
                    let _ = p.close(slot);
                }
                ChildFd::File(path) => {
                    if let Ok(f) = p.open_file(path, cake_platform::FileOpenMode::Write) {
                        if let Ok(s) = p.dup(slot) {
                            saved.push((slot, s));
                        }
                        let _ = p.dup2(f, slot);
                        let _ = p.close(f);
                    }
                }
                _ => {}
            }
        }
        for fd in &fds.owned {
            let _ = p.close(*fd);
        }
        let result = f(self);
        for (slot, s) in saved {
            let _ = p.dup2(s, slot);
            let _ = p.close(s);
        }
        result
    }
}
