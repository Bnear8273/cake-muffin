//! The shell evaluator: walks the AST and executes it.
//!
//! M2 replaces the M0 argv-splitter with the full parser and adds control
//! flow, expansions, redirections, pipelines, builtins and functions.

use alloc::borrow::ToOwned;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use cake_blacklist::CommandBlacklist;
use cake_env::{EnvStack, EnvVar};
use cake_platform::{ChildFd, Fd, ProcessHandle, SpawnConfig, WaitOptions, WaitStatus};
use cake_proc::ProcStatus;
use cake_syntax::{
    AndOrList, AndOrOp, AssignmentValue, CStyleForCommand, CaseCommand, Command, CommandKind,
    CompleteCommand, CoprocCommand, ForCommand, List, Pipeline, Program, SelectCommand, Separator,
    SimpleCommand, WhileCommand, WordPart, parse,
};

use crate::arith::eval_arith;
use crate::builtins;
use crate::cond::eval_cond;
use crate::expand::{ExpandCtx, expand_word, expand_word_quoted};
use crate::glob::glob_match_ext;
use crate::redirect::{CommandFds, setup_redirects};
use crate::resolve::{CommandSpec, resolve_command};

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

/// Signal from `break` / `continue`, consumed by the enclosing loop.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LoopControl {
    /// `true` = break, `false` = continue.
    pub(crate) is_break: bool,
    /// How many levels to affect.
    pub(crate) depth: u32,
}

/// A background job tracked by the shell (`jobs`/`wait`/`$!`).
#[derive(Debug, Clone)]
pub(crate) struct JobEntry {
    pub(crate) job_id: usize,
    pub(crate) handle: ProcessHandle,
    /// Textual form of the command, for `jobs` output.
    pub(crate) cmd: String,
    /// `Some` once the job has finished.
    pub(crate) status: Option<ProcStatus>,
}

/// Byte offsets where each line of `src` starts (for `$LINENO`).
fn line_starts_of(src: &str) -> Vec<usize> {
    let mut starts = alloc::vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// 1-based line number of a byte offset.
fn line_number(line_starts: &[usize], offset: usize) -> u32 {
    match line_starts.binary_search(&offset) {
        Ok(i) => (i + 1) as u32,
        Err(i) => i as u32,
    }
}

/// Rebuild the original text of a word from its parts (for `jobs` output).
pub(crate) fn word_text(w: &cake_syntax::Word) -> String {
    use cake_syntax::WordPart;
    let mut s = String::new();
    for part in &w.parts {
        match part {
            WordPart::Literal(t, _) => s.push_str(t),
            WordPart::SingleQuoted(t, _) => {
                s.push('\'');
                s.push_str(t);
                s.push('\'');
            }
            WordPart::AnsiCQuoted(t, _) => {
                s.push_str("$'");
                s.push_str(t);
                s.push('\'');
            }
            WordPart::DoubleQuoted(parts, _) => {
                s.push('"');
                for p in parts {
                    s.push_str(&part_text(p));
                }
                s.push('"');
            }
            WordPart::Parameter(p, _) => s.push_str(&p.text),
            WordPart::CommandSubst(t, _) => {
                s.push_str("$(");
                s.push_str(t);
                s.push(')');
            }
            WordPart::ArithExpansion(t, _) => {
                s.push_str("$((");
                s.push_str(t);
                s.push_str("))");
            }
            WordPart::Tilde(t, _) | WordPart::Brace(t, _) | WordPart::ProcessSubst(t, _) => {
                s.push_str(t)
            }
        }
    }
    s
}

fn part_text(p: &cake_syntax::WordPart) -> String {
    match p {
        cake_syntax::WordPart::DoubleQuoted(parts, _) => {
            let mut s = String::new();
            for inner in parts {
                s.push_str(&part_text(inner));
            }
            s
        }
        _ => word_text(&cake_syntax::Word {
            parts: alloc::vec![p.clone()],
            span: cake_syntax::Span::new(0, 0),
        }),
    }
}

/// What a `trap` entry reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapTrigger {
    Signal(cake_platform::Signal),
    Exit,
    Err,
    Debug,
}

/// Shell options toggled by `shopt`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShoptBits {
    /// `nullglob`: unmatched glob patterns expand to nothing.
    pub nullglob: bool,
    /// `dotglob`: glob patterns match hidden files.
    pub dotglob: bool,
    /// `nocaseglob`: glob matching ignores case.
    pub nocaseglob: bool,
    /// `extglob`: extended glob patterns `?(...)` `*(...)` `+(...)` `@(...)` `!(...)`.
    pub extglob: bool,
    /// `globstar`: `**` matches any number of directories recursively.
    pub globstar: bool,
}

/// The shell evaluator.
#[derive(Debug)]
pub struct Executor {
    pub env: EnvStack,
    pub last_status: ProcStatus,
    /// Positional parameters; `positional[0]` is `$0`.
    pub positional: Vec<String>,
    pub functions: BTreeMap<String, Command>,
    /// Command aliases (`alias name=value`). Expanded for command-position
    /// words when [`Executor::expand_aliases`] is set.
    pub aliases: BTreeMap<String, String>,
    /// Whether to expand aliases. Mirrors bash: aliases expand in interactive
    /// shells but not in `-c`/script mode by default.
    pub expand_aliases: bool,
    /// Set when `exit` is called; checked by the outer eval loop.
    exit_requested: Option<i32>,
    /// Set by `break`/`continue`; consumed by the enclosing loop.
    pub(crate) loop_control: Option<LoopControl>,
    /// Set by `return`; consumed by the enclosing function/source.
    pub(crate) return_requested: Option<i32>,
    /// Nesting depth of loops (for `break`/`continue` validity).
    pub(crate) loop_depth: usize,
    /// Nesting depth of functions/sourced files (for `return` validity).
    pub(crate) fn_depth: usize,
    /// The shell's own pid (`$$`). Set by the driver.
    pub shell_pid: i32,
    /// Background jobs not yet finished (`jobs`/`wait`/`$!`).
    pub(crate) background: Vec<JobEntry>,
    /// Monotonic job counter for `[n]` job ids.
    next_job_id: usize,
    /// Pid of the most recent background job (`$!`).
    pub last_bg_pid: i32,
    /// Whether this shell is interactive (affects `fg`/`bg` error text).
    pub interactive: bool,
    /// LCG state for `$RANDOM`.
    pub(crate) random_state: u32,
    /// Shell start time (epoch seconds) for `$SECONDS`.
    pub(crate) start_time: i64,
    /// Line number of the command currently being evaluated (`$LINENO`).
    pub(crate) cmd_lineno: u32,
    /// Fds kept open for `<(cmd)`/`>(cmd)` process substitutions.
    pub(crate) proc_subst: Vec<cake_platform::Fd>,
    /// Inside a forked sub-shell: inherited DEBUG traps are suppressed
    /// (bash only runs traps registered inside the sub-shell).
    pub(crate) in_subshell: bool,
    /// Commands that were not found (persisted by the driver).
    pub blacklist: CommandBlacklist,
    /// `set -e`: exit on a failing simple command (outside exempt contexts).
    pub errexit: bool,
    /// `set -u`: unset variable expansions are an error.
    pub nounset: bool,
    /// `set -f`: disable globbing.
    pub noglob: bool,
    /// `set -o pipefail`: a pipeline's status is the last non-zero element.
    pub pipefail: bool,
    /// `set -E`: ERR traps propagate into functions.
    pub errtrace: bool,
    /// `set -T`: DEBUG traps propagate into functions.
    pub functrace: bool,
    /// `shopt` toggles.
    pub shopt: ShoptBits,
    /// `trap` entries in the order registered.
    pub traps: Vec<(TrapTrigger, String)>,
    /// How many enclosing contexts exempt the current command from `errexit`.
    errexit_suppress: usize,
    /// Set when `errexit` fired; short-circuits the rest of evaluation.
    errexit_pending: Option<i32>,
    /// Suppresses nested trap execution.
    in_trap: bool,
    /// Directory stack for `pushd`/`popd`.
    pub(crate) dir_stack: Vec<String>,
}

impl Executor {
    pub fn new(env: EnvStack) -> Self {
        Self {
            env,
            last_status: ProcStatus::NotStarted,
            positional: alloc::vec!["cake".into()],
            functions: BTreeMap::new(),
            aliases: BTreeMap::new(),
            expand_aliases: false,
            exit_requested: None,
            loop_control: None,
            return_requested: None,
            loop_depth: 0,
            fn_depth: 0,
            shell_pid: 0,
            background: Vec::new(),
            next_job_id: 1,
            last_bg_pid: 0,
            interactive: false,
            random_state: cake_platform::try_get()
                .map(|p| p.time_seconds() as u32 ^ 0x9e3779b9)
                .unwrap_or(0x9e3779b9),
            start_time: cake_platform::try_get()
                .map(|p| p.time_seconds())
                .unwrap_or(0),
            cmd_lineno: 1,
            proc_subst: Vec::new(),
            in_subshell: false,
            blacklist: CommandBlacklist::new(),
            errexit: false,
            nounset: false,
            noglob: false,
            pipefail: false,
            errtrace: false,
            functrace: false,
            shopt: ShoptBits::default(),
            traps: Vec::new(),
            errexit_suppress: 0,
            errexit_pending: None,
            in_trap: false,
            dir_stack: Vec::new(),
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

    /// Reap any exited background jobs, recording their status.
    pub fn reap_background(&mut self) {
        for job in &mut self.background {
            if job.status.is_some() {
                continue;
            }
            if let Ok(WaitStatus::Exited(code)) =
                cake_platform::get().wait(&job.handle, WaitOptions::NOHANG)
            {
                job.status = Some(ProcStatus::Exit(code as i32));
            }
        }
    }

    /// Reap a specific background job, returning its final status.
    pub fn reap_job(&mut self, pid: i32) -> Option<ProcStatus> {
        let idx = self.background.iter().position(|j| j.handle.pid() == pid)?;
        let handle = self.background[idx].handle;
        let status = match self.background[idx].status {
            Some(s) => s,
            None => self.wait_for(&handle),
        };
        self.background.remove(idx);
        Some(status)
    }

    // --- entry point -----------------------------------------------------

    /// Evaluate a command string (used by `cake -c` and the interactive
    /// reader). Leaves any `exit` request intact so the caller can observe it
    /// via [`Self::exit_requested`].
    pub fn eval_str(&mut self, src: &str) -> EvalOutcome {
        self.run_pending_signal_traps();
        match parse(src) {
            Ok(prog) => {
                let line_starts = line_starts_of(src);
                let status = self.eval_program(&prog, &line_starts);
                // Process-substitution fds live for one evaluation.
                for fd in self.proc_subst.drain(..) {
                    let _ = cake_platform::get().close(fd);
                }
                self.last_status = status;
                // A pending `set -e` exit is reported through the status; it
                // is cleared here so a fresh input starts clean (interactive).
                let final_status = self
                    .errexit_pending
                    .take()
                    .map(ProcStatus::Exit)
                    .unwrap_or(status);
                let final_status = self
                    .exit_requested()
                    .map(ProcStatus::Exit)
                    .unwrap_or(final_status);
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

    fn eval_program(&mut self, prog: &Program, line_starts: &[usize]) -> ProcStatus {
        let mut status = ProcStatus::Exit(0);
        for cc in &prog.commands {
            if self.exit_requested.is_some() || self.errexit_pending.is_some() {
                break;
            }
            self.cmd_lineno = line_number(line_starts, cc.span.start as usize);
            status = self.eval_complete(cc);
            self.run_pending_signal_traps();
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
            if self.exit_requested.is_some() || self.errexit_pending.is_some() {
                break;
            }
            status = self.eval_and_or(item);
            // `break`/`continue`/`return` skip the rest of this list.
            if self.loop_control.is_some() || self.return_requested.is_some() {
                break;
            }
        }
        status
    }

    /// Evaluate one pipeline, then check `set -e`. A failing pipeline fires
    /// unless it is negated (`!`) or inside an exempt context (`errexit_suppress`).
    fn eval_pipeline_checked(&mut self, pipeline: &Pipeline) -> ProcStatus {
        let status = self.eval_pipeline(pipeline);
        let last_simple = matches!(
            pipeline.commands.last().map(|c| &c.kind),
            Some(CommandKind::Simple(_)) | Some(CommandKind::Empty)
        );
        self.maybe_fire_errexit(&status, pipeline.negated, last_simple);
        status
    }

    /// `set -e` trigger point (and `trap ERR` hook).
    ///
    /// `errexit` fires for any failing command; the ERR trap fires only when
    /// the failing command is a simple command (bash: a failing `if`/`for`/
    /// subshell does not run the ERR trap).
    fn maybe_fire_errexit(&mut self, status: &ProcStatus, negated: bool, last_simple: bool) {
        if self.errexit_pending.is_some() {
            return;
        }
        if self.errexit && !negated && self.errexit_suppress == 0 && !status.success() {
            self.errexit_pending = Some(status.status_code());
        }
        if last_simple
            && !negated
            && self.errexit_suppress == 0
            && !status.success()
            && (self.fn_depth == 0 || self.errtrace)
        {
            self.run_trap(TrapTrigger::Err);
        }
    }

    /// Run the command registered for `trigger`, if any. Nested traps are
    /// suppressed (`in_trap`) and a pending `errexit`/`exit` survives the trap.
    pub(crate) fn run_trap(&mut self, trigger: TrapTrigger) {
        if self.in_trap {
            return;
        }
        let cmd = self
            .traps
            .iter()
            .find(|(t, _)| *t == trigger)
            .map(|(_, c)| c.clone());
        if let Some(cmd) = cmd {
            self.in_trap = true;
            let saved_errexit = self.errexit_pending.take();
            let saved_exit = self.exit_requested.take();
            let _ = self.eval_str(&cmd);
            self.in_trap = false;
            if saved_errexit.is_some() {
                self.errexit_pending = saved_errexit;
            }
            if saved_exit.is_some() {
                self.exit_requested = saved_exit;
            }
        }
    }

    /// Run `trap ... EXIT` handlers once (they are consumed), then drop them.
    /// Called by the driver when the shell exits and by sub-shells/background
    /// jobs when they finish.
    pub fn run_exit_traps(&mut self) {
        self.run_exit_traps_except(&[]);
    }

    /// Like [`Self::run_exit_traps`], but skips handlers already registered
    /// before the current sub-shell started (bash: an inherited EXIT trap is
    /// not re-run by a sub-shell).
    pub(crate) fn run_exit_traps_except(&mut self, inherited: &[String]) {
        let cmds: Vec<String> = self
            .traps
            .iter()
            .filter(|(t, _)| *t == TrapTrigger::Exit)
            .filter(|(_, c)| !inherited.iter().any(|i| i == c))
            .map(|(_, c)| c.clone())
            .collect();
        self.traps.retain(|(t, _)| *t != TrapTrigger::Exit);
        // A pending `exit` would short-circuit the trap commands.
        let saved_exit = self.exit_requested.take();
        for c in cmds {
            if self.in_trap {
                break;
            }
            self.in_trap = true;
            let _ = self.eval_str(&c);
            self.in_trap = false;
        }
        if saved_exit.is_some() {
            self.exit_requested = saved_exit;
        }
    }

    /// Execute traps registered for real signals that arrived since last
    /// check. Called at evaluation boundaries.
    pub(crate) fn run_pending_signal_traps(&mut self) {
        for sig in cake_platform::get().drain_received_signals() {
            self.run_trap(TrapTrigger::Signal(sig));
        }
    }

    fn eval_and_or(&mut self, aol: &AndOrList) -> ProcStatus {
        // In a `&&`/`||` chain every pipeline except the final one is exempt
        // from `set -e` (bash: the command following the final operator is not).
        let final_idx = aol.rest.len();
        let mut status = if final_idx == 0 {
            self.eval_pipeline_checked(&aol.first)
        } else {
            self.errexit_suppress += 1;
            let st = self.eval_pipeline_checked(&aol.first);
            self.errexit_suppress -= 1;
            st
        };
        for (i, (op, pipeline)) in aol.rest.iter().enumerate() {
            if self.exit_requested.is_some()
                || self.loop_control.is_some()
                || self.return_requested.is_some()
                || self.errexit_pending.is_some()
            {
                break;
            }
            let need_run = match op {
                AndOrOp::AndAnd => status.success(),
                AndOrOp::OrOr => !status.success(),
            };
            if need_run {
                if i < final_idx - 1 {
                    self.errexit_suppress += 1;
                }
                status = self.eval_pipeline_checked(pipeline);
                if i < final_idx - 1 {
                    self.errexit_suppress -= 1;
                }
            }
        }
        self.last_status = status;
        status
    }

    /// `list &` — run asynchronously.
    ///
    /// # Portability
    ///
    /// Uses [`run_in_child`] which is `fork()`-based on Unix. The raw-pointer
    /// reborrow (`&mut *self as *mut Executor`) only works because `fork`
    /// snapshots heap memory; a future Windows backend must instead spawn a
    /// fresh `cake -c` process or use `CreateProcess` + IPC (see the trait
    /// doc on [`Platform::run_in_child`]).
    fn eval_and_or_background(&mut self, aol: &AndOrList) -> ProcStatus {
        let list = aol.clone();
        let cmd_text = self.and_or_to_text(aol);
        let job_id = self.next_job_id;
        self.next_job_id += 1;
        let exec = &mut *self as *mut Executor;
        let result = cake_platform::get().run_in_child(&mut move || {
            let exec = unsafe { &mut *exec };
            let inherited: Vec<String> = exec
                .traps
                .iter()
                .filter(|(t, _)| *t == TrapTrigger::Exit)
                .map(|(_, c)| c.clone())
                .collect();
            let st = exec.eval_and_or(&list).status_code();
            exec.run_exit_traps_except(&inherited);
            st
        });
        if let Ok(h) = result {
            self.background.push(JobEntry {
                job_id,
                handle: h,
                cmd: cmd_text,
                status: None,
            });
            self.last_bg_pid = h.pid();
        }
        ProcStatus::Exit(0)
    }

    /// Rebuild a short textual form of the command for `jobs` output.
    fn and_or_to_text(&self, aol: &AndOrList) -> String {
        let mut s = String::new();
        for (i, cmd) in aol.first.commands.iter().enumerate() {
            if i > 0 {
                s.push('|');
            }
            s.push_str(&self.command_to_text(cmd));
        }
        s
    }

    fn command_to_text(&self, cmd: &Command) -> String {
        use cake_syntax::{AssignmentValue, RedirectTarget};
        match &cmd.kind {
            CommandKind::Simple(sc) => {
                let mut parts: Vec<String> = Vec::new();
                for a in &sc.assignments {
                    parts.push(alloc::format!(
                        "{}{}",
                        a.name,
                        match &a.value {
                            AssignmentValue::Word(w) => word_text(w),
                            AssignmentValue::Array(ws) => {
                                let inner: Vec<String> = ws.iter().map(word_text).collect();
                                alloc::format!("({})", inner.join(" "))
                            }
                            AssignmentValue::Index { index, value } => {
                                alloc::format!("[{}]{}", word_text(index), word_text(value))
                            }
                        }
                    ));
                }
                for w in &sc.words {
                    parts.push(word_text(w));
                }
                for r in &cmd.redirects {
                    let mut redir = String::new();
                    if let Some(fd) = r.fd {
                        redir.push_str(&alloc::string::ToString::to_string(&fd));
                    }
                    redir.push_str(match r.kind {
                        cake_syntax::RedirectKind::Write => ">",
                        cake_syntax::RedirectKind::Append => ">>",
                        cake_syntax::RedirectKind::Read => "<",
                        cake_syntax::RedirectKind::ReadWrite => "<>",
                        cake_syntax::RedirectKind::DupInput => "<&",
                        cake_syntax::RedirectKind::DupOutput => ">&",
                        cake_syntax::RedirectKind::Heredoc => "<<",
                        cake_syntax::RedirectKind::HereString => "<<<",
                        cake_syntax::RedirectKind::Clobber => ">|",
                        cake_syntax::RedirectKind::AndOut => "&>",
                        cake_syntax::RedirectKind::AndAppend => "&>>",
                    });
                    match &r.target {
                        RedirectTarget::Word(w) => redir.push_str(&word_text(w)),
                        RedirectTarget::Fd(n) => {
                            redir.push_str(&alloc::string::ToString::to_string(n))
                        }
                        RedirectTarget::Close => redir.push('-'),
                        RedirectTarget::HereString(w) => redir.push_str(&word_text(w)),
                        RedirectTarget::Heredoc { .. } => {}
                    }
                    parts.push(redir);
                }
                parts.join(" ")
            }
            _ => "?".into(),
        }
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
        // For `set -o pipefail`: the last non-zero element status.
        let mut pipe_status: Option<i32> = None;

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
            let prev_read_old = prev_read;
            if let Some(pr) = prev_read_old
                && fds.stdin == ChildFd::Inherit
            {
                fds.stdin = ChildFd::Fd(pr);
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
                    if !st.success() {
                        pipe_status = Some(st.status_code());
                    }
                }
                Ok(EvalResult::Spawned(h)) => {
                    // A spawned process: wait for its final status later. If
                    // this is the last element, its status is the pipeline's.
                    if is_last {
                        last_status = self.wait_for(&h);
                        if !last_status.success() {
                            pipe_status = Some(last_status.status_code());
                        }
                    } else {
                        handles.push(h);
                    }
                }
                Err(e) => {
                    self.report_error(&e);
                    last_status = ProcStatus::Exit(127);
                    pipe_status = Some(127);
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
            if let Some(pr) = prev_read_old {
                let _ = cake_platform::get().close(pr);
            }
        }

        for h in &handles {
            let _ = self.wait_for(h);
        }

        if self.pipefail
            && let Some(code) = pipe_status
        {
            last_status = ProcStatus::Exit(code);
        }

        if pipeline.negated {
            last_status = ProcStatus::Exit(if last_status.success() { 1 } else { 0 });
        }
        last_status
    }

    fn setup_cmd_fds(&mut self, cmd: &Command) -> Result<CommandFds, String> {
        let mut ctx = self.ctx();
        setup_redirects(&mut ctx, &cmd.redirects)
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
            CommandKind::CStyleFor(csf) => {
                let st = self.apply_fds_in_parent(fds, |exec| exec.eval_c_style_for(csf));
                Ok(EvalResult::Done(st))
            }
            CommandKind::Case(cs) => {
                let st = self.apply_fds_in_parent(fds, |exec| exec.eval_case(cs));
                Ok(EvalResult::Done(st))
            }
            CommandKind::Select(sc) => {
                let st = self.apply_fds_in_parent(fds, |exec| exec.eval_select(sc));
                Ok(EvalResult::Done(st))
            }
            CommandKind::Coproc(cc) => {
                let st = self.apply_fds_in_parent(fds, |exec| exec.eval_coproc(cc));
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
                // # Portability
                //
                // Like background jobs, subshells run in a `fork`ed child via
                // [`Platform::run_in_child`]; the raw-pointer reborrow only
                // works on Unix. A Windows backend must spawn a fresh process
                // instead (see the trait doc).
                let body = sub.body.clone();
                let fds = fds.clone();
                let exec = &mut *self;
                let handle = cake_platform::get().run_in_child(&mut move || {
                    let inherited: Vec<String> = exec
                        .traps
                        .iter()
                        .filter(|(t, _)| *t == TrapTrigger::Exit)
                        .map(|(_, c)| c.clone())
                        .collect();
                    exec.in_subshell = true;
                    let st = exec
                        .apply_fds_in_parent(&fds, |e| e.eval_list(&body))
                        .status_code();
                    exec.run_exit_traps_except(&inherited);
                    st
                });
                match handle {
                    Ok(h) => Ok(EvalResult::Spawned(h)),
                    Err(e) => Err(alloc::format!("cake: subshell: {e}")),
                }
            }
            CommandKind::Function(fn_cmd) => {
                if self.functrace {
                    self.run_trap(TrapTrigger::Debug);
                }
                self.functions
                    .insert(fn_cmd.name.clone(), (*fn_cmd.body).clone());
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

    fn eval_simple(
        &mut self,
        sc: &SimpleCommand,
        fds: &mut CommandFds,
    ) -> Result<EvalResult, String> {
        // `trap ... DEBUG` fires before every simple command (not inside
        // functions unless `set -T`).
        if (self.fn_depth == 0 || self.functrace) && !self.in_subshell {
            self.run_trap(TrapTrigger::Debug);
        }
        // Prefix assignments. Arrays and indexed assignments always take
        // effect in the current shell; scalar assignments are temporary when
        // followed by a command.
        let mut temp_assigns: Vec<(String, String)> = Vec::new();
        let mut shell_assigns: Vec<(String, String)> = Vec::new();
        for a in &sc.assignments {
            match &a.value {
                AssignmentValue::Word(w) => {
                    let mut ctx = self.ctx();
                    let v = expand_word_quoted(&mut ctx, w)?;
                    if sc.words.is_empty() {
                        shell_assigns.push((a.name.clone(), v));
                    } else {
                        temp_assigns.push((a.name.clone(), v));
                    }
                }
                AssignmentValue::Array(words) => {
                    let mut values: Vec<String> = Vec::new();
                    let mut ctx = self.ctx();
                    for w in words {
                        values.extend(expand_word(&mut ctx, w)?);
                    }
                    self.env
                        .set(&a.name, cake_env::EnvVar::new_list(values))
                        .map_err(|e| alloc::format!("cake: {e}"))?;
                }
                AssignmentValue::Index { index, value } => {
                    let mut ctx = self.ctx();
                    let expanded_idx = expand_word_quoted(&mut ctx, index)?;
                    let idx_str = crate::arith::eval_arith_value(ctx.env, &expanded_idx)?;
                    let idx: usize = idx_str.parse().unwrap_or(0);
                    let v = expand_word_quoted(&mut ctx, value)?;
                    set_indexed(self, &a.name, idx, v).map_err(|e| alloc::format!("cake: {e}"))?;
                }
            }
        }

        // Expand the command words.
        let mut argv: Vec<String> = Vec::new();
        {
            let mut ctx = self.ctx();
            for w in &sc.words {
                let fields = expand_word(&mut ctx, w)?;
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

        // Alias expansion (interactive only). Replaces the command word with
        // the alias value and keeps the rest of the argv. Skipped when the
        // first word was quoted (`'ll'` doesn't expand), matching bash.
        let first_quoted = matches!(
            sc.words.first().and_then(|w| w.parts.first()),
            Some(WordPart::SingleQuoted(..) | WordPart::DoubleQuoted(..))
        );
        let argv = if !first_quoted {
            let expanded = self.expand_aliases(argv);
            if expanded.is_empty() {
                return Ok(EvalResult::Done(ProcStatus::Exit(0)));
            }
            expanded
        } else {
            argv
        };

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

        let saved_positional = core::mem::replace(&mut self.positional, argv.to_vec());
        self.env.push_scope();
        for (n, v) in &temp_assigns {
            let _ = self.env.set(n, EnvVar::new(v.clone()));
        }
        let mut fds = CommandFds::default();
        self.fn_depth += 1;
        let status = self.eval_command(&body, &mut fds).map(|r| match r {
            EvalResult::Done(st) => {
                // `return` inside the function body overrides the status.
                match self.return_requested.take() {
                    Some(code) => ProcStatus::Exit(code),
                    None => st,
                }
            }
            EvalResult::Spawned(h) => self.wait_for(&h),
        });
        self.fn_depth -= 1;
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

    pub(crate) fn wait_for(&mut self, handle: &ProcessHandle) -> ProcStatus {
        match cake_platform::get().wait(handle, WaitOptions::NONE) {
            Ok(WaitStatus::Exited(code)) => ProcStatus::Exit(code as i32),
            Ok(WaitStatus::Signaled(sig)) => {
                ProcStatus::Signal(cake_platform::get().signal_number(sig))
            }
            Ok(_) => ProcStatus::Exit(1),
            Err(_) => ProcStatus::Exit(127),
        }
    }

    // --- control flow ----------------------------------------------------

    fn eval_if(&mut self, ifc: &cake_syntax::IfCommand) -> ProcStatus {
        for clause in &ifc.clauses {
            self.errexit_suppress += 1;
            let st = self.eval_and_or(&clause.cond);
            self.errexit_suppress -= 1;
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
        self.loop_depth += 1;
        let mut result = ProcStatus::Exit(0);
        loop {
            self.errexit_suppress += 1;
            let st = self.eval_and_or(&w.cond);
            self.errexit_suppress -= 1;
            if self.exit_requested.is_some() {
                result = st;
                break;
            }
            let do_body = if until { !st.success() } else { st.success() };
            if !do_body {
                break;
            }
            self.eval_list(&w.body);

            if let Some(lc) = self.loop_control.take() {
                if lc.is_break {
                    if lc.depth > 1 {
                        self.loop_control = Some(LoopControl {
                            is_break: true,
                            depth: lc.depth - 1,
                        });
                    }
                    break;
                }
                // continue: skip to the next condition check
                if lc.depth > 1 {
                    self.loop_control = Some(LoopControl {
                        is_break: false,
                        depth: lc.depth - 1,
                    });
                }
            }
        }
        self.loop_depth -= 1;
        result
    }

    fn eval_for(&mut self, fc: &ForCommand) -> ProcStatus {
        let words: Vec<String> = match &fc.in_words {
            Some(in_words) => {
                let mut out = Vec::new();
                let mut ctx = self.ctx();
                for w in in_words {
                    if let Ok(fields) = expand_word(&mut ctx, w) {
                        out.extend(fields);
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
        self.loop_depth += 1;
        for w in words {
            let _ = self.env.set(&fc.var, EnvVar::new(w));
            self.eval_list(&fc.body);
            if self.exit_requested.is_some() {
                break;
            }
            if let Some(lc) = self.loop_control.take() {
                if lc.is_break {
                    if lc.depth > 1 {
                        self.loop_control = Some(LoopControl {
                            is_break: true,
                            depth: lc.depth - 1,
                        });
                    }
                    break;
                }
                // continue: move to the next word
                if lc.depth > 1 {
                    self.loop_control = Some(LoopControl {
                        is_break: false,
                        depth: lc.depth - 1,
                    });
                }
            }
        }
        self.loop_depth -= 1;
        ProcStatus::Exit(0)
    }

    fn eval_c_style_for(&mut self, csf: &CStyleForCommand) -> ProcStatus {
        self.loop_depth += 1;

        // 1. Evaluate the initializer (once)
        if !csf.init.is_empty() {
            let init_result = eval_arith(self, &csf.init);
            if self.exit_requested.is_some() {
                self.loop_depth -= 1;
                return init_result;
            }
        }

        let mut result = ProcStatus::Exit(0);

        loop {
            // 2. Evaluate the condition (if non-empty). Empty cond = always true
            if !csf.cond.is_empty() {
                self.errexit_suppress += 1;
                let cond_result = eval_arith(self, &csf.cond);
                self.errexit_suppress -= 1;

                if self.exit_requested.is_some() {
                    result = cond_result;
                    break;
                }

                // In bash, `(( 0 ))` has exit status 1 (false), `(( nonzero ))` is 0 (true)
                if !cond_result.success() {
                    break;
                }
            }
            // If cond is empty, we always enter the body (infinite loop unless break)

            // 3. Execute the body
            self.eval_list(&csf.body);

            // 4. Handle break/continue
            if let Some(lc) = self.loop_control.take() {
                if lc.is_break {
                    if lc.depth > 1 {
                        self.loop_control = Some(LoopControl {
                            is_break: true,
                            depth: lc.depth - 1,
                        });
                    }
                    break;
                }
                // continue: skip to the increment, then re-check condition
                if lc.depth > 1 {
                    self.loop_control = Some(LoopControl {
                        is_break: false,
                        depth: lc.depth - 1,
                    });
                }
                // Fall through to the increment (do NOT skip it on continue)
            }

            // 5. Evaluate the increment (if non-empty)
            if !csf.incr.is_empty() {
                let _ = eval_arith(self, &csf.incr);
                // eval_arith already applies side effects to self.env
            }
        }

        self.loop_depth -= 1;
        result
    }

    fn eval_case(&mut self, cs: &CaseCommand) -> ProcStatus {
        let word = {
            let mut ctx = self.ctx();
            expand_word_quoted(&mut ctx, &cs.word).unwrap_or_default()
        };
        for arm in &cs.arms {
            for pat in &arm.patterns {
                let pat_str = {
                    let mut ctx = self.ctx();
                    // Case patterns are single words: no field splitting and
                    // no pathname expansion (`*` stays a pattern).
                    expand_word_quoted(&mut ctx, pat).unwrap_or_default()
                };
                if glob_match_ext(&pat_str, &word, false, self.shopt.extglob) {
                    return self.eval_list(&arm.body);
                }
            }
        }
        ProcStatus::Exit(0)
    }

    fn eval_select(&mut self, sc: &SelectCommand) -> ProcStatus {
        let words: Vec<String> = match &sc.in_words {
            Some(in_words) => {
                let mut out = Vec::new();
                let mut ctx = self.ctx();
                for w in in_words {
                    if let Ok(fields) = expand_word(&mut ctx, w) {
                        out.extend(fields);
                    }
                }
                out
            }
            None => {
                // `select var; do` == `select var in "$@"`
                if self.positional.len() > 1 {
                    self.positional[1..].to_vec()
                } else {
                    Vec::new()
                }
            }
        };

        self.loop_depth += 1;
        loop {
            // Print the numbered menu
            for (i, item) in words.iter().enumerate() {
                let _ = cake_platform::get()
                    .write(1, alloc::format!("{}) {}\n", i + 1, item).as_bytes());
            }

            // Print the prompt: $PS3 or "#? "
            let prompt = self
                .env
                .get("PS3")
                .map(|v| v.value().to_owned())
                .unwrap_or_else(|| "#? ".to_owned());
            let _ = cake_platform::get().write(1, prompt.as_bytes());

            // Read a line from stdin
            let line = match builtins::read_line(false) {
                Ok(l) => l,
                Err(_) => break, // EOF → exit loop
            };

            // Empty input → name is empty, skip body
            if line.trim().is_empty() {
                let _ = self.env.set(&sc.var, EnvVar::new(String::new()));
                continue;
            }

            // Parse the number
            let choice: usize = match line.trim().parse() {
                Ok(n) if n >= 1 && n <= words.len() => n,
                _ => {
                    let _ = self.env.set(&sc.var, EnvVar::new(String::new()));
                    // Bash: invalid input prints an error and skips body
                    continue;
                }
            };

            let _ = self
                .env
                .set(&sc.var, EnvVar::new(words[choice - 1].clone()));
            self.eval_list(&sc.body);

            if self.exit_requested.is_some() {
                break;
            }

            if let Some(lc) = self.loop_control.take() {
                if lc.is_break {
                    if lc.depth > 1 {
                        self.loop_control = Some(LoopControl {
                            is_break: true,
                            depth: lc.depth - 1,
                        });
                    }
                    break;
                }
                if lc.depth > 1 {
                    self.loop_control = Some(LoopControl {
                        is_break: false,
                        depth: lc.depth - 1,
                    });
                }
            }
        }

        self.loop_depth -= 1;
        ProcStatus::Exit(0)
    }

    fn eval_coproc(&mut self, cc: &CoprocCommand) -> ProcStatus {
        // 1. Create two pipes
        let p = cake_platform::get();
        let (c2p_read, c2p_write) = match p.pipe(false) {
            Ok(p) => p,
            Err(e) => {
                self.report_error(&alloc::format!("cake: coproc: pipe: {e}"));
                return ProcStatus::Exit(1);
            }
        };
        let (p2c_read, p2c_write) = match p.pipe(false) {
            Ok(p) => p,
            Err(e) => {
                let _ = p.close(c2p_read);
                let _ = p.close(c2p_write);
                self.report_error(&alloc::format!("cake: coproc: pipe: {e}"));
                return ProcStatus::Exit(1);
            }
        };

        // 2. Determine the variable name
        let var_name = cc.name.clone().unwrap_or_else(|| "COPROC".to_owned());

        // 3. Set up CommandFds for the child:
        //    child stdin  = p2c_read  (reads what parent writes)
        //    child stdout = c2p_write (writes what parent reads)
        let mut child_fds = crate::redirect::CommandFds {
            stdin: ChildFd::Fd(p2c_read),
            stdout: ChildFd::Fd(c2p_write),
            owned: vec![p2c_read, c2p_write],
            ..Default::default()
        };

        // 4. Fork the child
        let body = (*cc.body).clone();
        let exec = &mut *self as *mut Executor;
        let handle = p.run_in_child(&mut move || {
            let exec = unsafe { &mut *exec };
            let inherited: Vec<String> = exec
                .traps
                .iter()
                .filter(|(t, _)| *t == TrapTrigger::Exit)
                .map(|(_, c)| c.clone())
                .collect();
            exec.in_subshell = true;
            let mut fds = child_fds.clone();
            let st = match exec.eval_command(&body, &mut fds) {
                Ok(result) => match result {
                    EvalResult::Done(s) => s.status_code(),
                    EvalResult::Spawned(h) => {
                        // Wait for the spawned process
                        match cake_platform::get().wait(&h, WaitOptions::NONE) {
                            Ok(WaitStatus::Exited(code)) => code as i32,
                            _ => 127,
                        }
                    }
                },
                Err(_) => 1,
            };
            exec.run_exit_traps_except(&inherited);
            st
        });

        match handle {
            Ok(h) => {
                // 5. Close the child-side fds in the parent
                let _ = p.close(c2p_read);
                let _ = p.close(p2c_write);

                // 6. Set the variable: an array with [0]=write_fd, [1]=read_fd
                //    $COPROC[0] = p2c_write (write to coprocess stdin)
                //    $COPROC[1] = c2p_read (read from coprocess stdout)
                let _ = self.env.set(
                    &var_name,
                    EnvVar::new_list(vec![
                        alloc::format!("{}", p2c_write), // [0] = write to coprocess
                        alloc::format!("{}", c2p_read),  // [1] = read from coprocess
                    ]),
                );

                // 7. Set $! to the coprocess pid
                self.last_bg_pid = h.pid();

                // Track the background job for `jobs`/`wait`
                let cmd_text =
                    alloc::format!("coproc {} {}", var_name, self.command_to_text(&cc.body));
                self.background.push(JobEntry {
                    job_id: self.next_job_id,
                    handle: h,
                    cmd: cmd_text,
                    status: None,
                });
                self.next_job_id += 1;

                ProcStatus::Exit(0)
            }
            Err(e) => {
                let _ = p.close(c2p_read);
                let _ = p.close(c2p_write);
                let _ = p.close(p2c_read);
                let _ = p.close(p2c_write);
                self.report_error(&alloc::format!("cake: coproc: {e}"));
                ProcStatus::Exit(1)
            }
        }
    }

    // --- helpers ---------------------------------------------------------

    fn ctx(&mut self) -> ExpandCtx<'_> {
        ExpandCtx {
            env: &mut self.env,
            last_status: self.last_status,
            positional: &self.positional,
            functions: &self.functions,
            aliases: &self.aliases,
            shell_pid: self.shell_pid,
            nounset: self.nounset,
            noglob: self.noglob,
            shopt: self.shopt,
            errexit: self.errexit,
            last_bg_pid: self.last_bg_pid,
            random_state: &mut self.random_state,
            start_time: self.start_time,
            lineno: self.cmd_lineno,
            parent_pid: cake_platform::try_get()
                .map(|p| p.parent_pid())
                .unwrap_or(0),
            proc_subst_fds: &mut self.proc_subst,
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
        for (slot, cfg) in [
            (0 as Fd, &fds.stdin),
            (1 as Fd, &fds.stdout),
            (2 as Fd, &fds.stderr),
        ] {
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

    /// Expand command-position words through the alias table, bash-style.
    ///
    /// Only the leading word is examined (and the next one when the alias
    /// value ends in a blank, e.g. `alias sudo='sudo '`). The alias value is
    /// re-read as shell words and expanded normally, so `$HOME`, quotes and
    /// tilde work inside it. Loop-protected against self-referential aliases.
    fn expand_aliases(&mut self, argv: Vec<String>) -> Vec<String> {
        if !self.expand_aliases {
            return argv;
        }
        let mut out: Vec<String> = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut i = 0usize;
        while i < argv.len() {
            let word = &argv[i];
            let expand_next = if is_alias_name(word) && !seen.contains(word.as_str()) {
                match self.aliases.get(word).cloned() {
                    Some(val) => {
                        seen.insert(word.as_str());
                        let trailing = val.ends_with(' ');
                        match cake_syntax::split_command_line(&val) {
                            Ok(sub) => {
                                for s in sub {
                                    let w = cake_syntax::word::parse_word(
                                        &s,
                                        cake_syntax::Span::new(0, 0),
                                    );
                                    let mut ctx = self.ctx();
                                    match expand_word(&mut ctx, &w) {
                                        Ok(fields) => out.extend(fields),
                                        Err(_) => out.push(s),
                                    }
                                }
                                trailing
                            }
                            Err(_) => {
                                out.push(word.clone());
                                false
                            }
                        }
                    }
                    None => {
                        out.push(word.clone());
                        false
                    }
                }
            } else {
                out.push(word.clone());
                false
            };
            i += 1;
            if !expand_next {
                break;
            }
        }
        if i < argv.len() {
            out.extend_from_slice(&argv[i..]);
        }
        out
    }
}

/// Whether a word is a plausible alias name: non-empty and not a path.
fn is_alias_name(word: &str) -> bool {
    !word.is_empty() && !word.contains('/') && !word.contains('\\')
}

/// Set a specific index of an array variable (growing it if needed).
fn set_indexed(exec: &mut Executor, name: &str, idx: usize, value: String) -> Result<(), String> {
    let mut values: Vec<String> = match exec.env.get(name) {
        Some(v) => v.values().to_vec(),
        None => Vec::new(),
    };
    if idx >= values.len() {
        values.resize(idx + 1, String::new());
    }
    values[idx] = value;
    exec.env
        .set(name, cake_env::EnvVar::new_list(values))
        .map_err(|e| alloc::format!("cake: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use cake_env::EnvStack;

    fn exec_with_aliases(aliases: &[(&str, &str)]) -> Executor {
        let mut e = Executor::new(EnvStack::new());
        e.expand_aliases = true;
        for (k, v) in aliases {
            e.aliases.insert((*k).to_owned(), (*v).to_owned());
        }
        e
    }

    #[test]
    fn expands_simple_alias_keeping_args() {
        let mut e = exec_with_aliases(&[("ls", "ls --color=auto")]);
        let argv = e.expand_aliases(vec!["ls".into(), "-la".into()]);
        assert_eq!(argv, ["ls", "--color=auto", "-la"]);
    }

    #[test]
    fn expands_parameter_in_alias_value() {
        let mut e = exec_with_aliases(&[("h", "echo $HOME")]);
        e.env.set("HOME", cake_env::EnvVar::new("/root")).unwrap();
        let argv = e.expand_aliases(vec!["h".into()]);
        assert_eq!(argv, ["echo", "/root"]);
    }

    #[test]
    fn trailing_space_chains_to_next_word() {
        let mut e = exec_with_aliases(&[("sudo", "sudo "), ("ls", "ls --color=auto")]);
        let argv = e.expand_aliases(vec!["sudo".into(), "ls".into()]);
        assert_eq!(argv, ["sudo", "ls", "--color=auto"]);
    }

    #[test]
    fn self_referential_alias_does_not_loop() {
        let mut e = exec_with_aliases(&[("a", "b"), ("b", "a")]);
        let argv = e.expand_aliases(vec!["a".into()]);
        assert_eq!(argv, ["b"]);
    }

    #[test]
    fn path_word_is_not_expanded() {
        let mut e = exec_with_aliases(&[("ls", "ls --color=auto")]);
        let argv = e.expand_aliases(vec!["/bin/ls".into()]);
        assert_eq!(argv, ["/bin/ls"]);
    }

    #[test]
    fn no_expansion_when_disabled() {
        let mut e = Executor::new(EnvStack::new());
        e.aliases.insert("ls".into(), "ls --color=auto".into());
        let argv = e.expand_aliases(vec!["ls".into()]);
        assert_eq!(argv, ["ls"]);
    }
}
