//! Redirection setup.
//!
//! Redirections are processed left-to-right like bash. Each produces the
//! final stdin/stdout/stderr for a command plus the parent-side fds that must
//! be closed after the child spawns (or after a builtin runs).

use alloc::string::String;
use alloc::vec::Vec;

use cake_platform::{ChildFd, FileOpenMode, Fd};
use cake_syntax::{Redirect, RedirectKind, RedirectTarget};

use crate::expand::{expand_redirect_word, ExpandCtx};

/// The resolved standard fds for one command.
#[derive(Clone)]
pub struct CommandFds {
    pub stdin: ChildFd,
    pub stdout: ChildFd,
    pub stderr: ChildFd,
    /// Parent-side fds to close after spawning the child / running the
    /// builtin.
    pub owned: Vec<Fd>,
}

impl Default for CommandFds {
    fn default() -> Self {
        Self {
            stdin: ChildFd::Inherit,
            stdout: ChildFd::Inherit,
            stderr: ChildFd::Inherit,
            owned: Vec::new(),
        }
    }
}

/// The slot a redirect writes to (0=stdin, 1=stdout, 2=stderr).
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Stdin,
    Stdout,
    Stderr,
}

impl Slot {
    fn from_fd(fd: u32, _default_output: bool) -> Result<Self, String> {
        match fd {
            0 => Ok(Slot::Stdin),
            1 => Ok(Slot::Stdout),
            2 => Ok(Slot::Stderr),
            other => Err(alloc::format!("fd {other} redirects are not supported yet")),
        }
    }
}

/// Process a command's redirect list into concrete fds.
pub fn setup_redirects(ctx: &mut ExpandCtx, redirects: &[Redirect]) -> Result<CommandFds, String> {
    let mut fds = CommandFds::default();
    for r in redirects {
        apply_redirect(ctx, r, &mut fds)?;
    }
    Ok(fds)
}

fn slot_mut(fds: &mut CommandFds, slot: Slot) -> &mut ChildFd {
    match slot {
        Slot::Stdin => &mut fds.stdin,
        Slot::Stdout => &mut fds.stdout,
        Slot::Stderr => &mut fds.stderr,
    }
}

fn apply_redirect(ctx: &mut ExpandCtx, r: &Redirect, fds: &mut CommandFds) -> Result<(), String> {
    let output_default = matches!(
        r.kind,
        RedirectKind::Write
            | RedirectKind::Append
            | RedirectKind::DupOutput
            | RedirectKind::Clobber
            | RedirectKind::AndOut
            | RedirectKind::AndAppend
    );
    let slot = Slot::from_fd(r.fd.unwrap_or(if output_default { 1 } else { 0 }), output_default)?;

    match (&r.kind, &r.target) {
        // File redirects: open in the parent, pass the fd to the child.
        (RedirectKind::Write | RedirectKind::Clobber, RedirectTarget::Word(w)) => {
            let path = expand_redirect_word(ctx, w)?;
            let fd = cake_platform::get()
                .open_file(&path, FileOpenMode::Write)
                .map_err(|e| alloc::format!("cake: {path}: {e}"))?;
            *slot_mut(fds, slot) = ChildFd::Fd(fd);
            fds.owned.push(fd);
        }
        (RedirectKind::Append, RedirectTarget::Word(w)) => {
            let path = expand_redirect_word(ctx, w)?;
            let fd = cake_platform::get()
                .open_file(&path, FileOpenMode::Append)
                .map_err(|e| alloc::format!("cake: {path}: {e}"))?;
            *slot_mut(fds, slot) = ChildFd::Fd(fd);
            fds.owned.push(fd);
        }
        (RedirectKind::Read, RedirectTarget::Word(w)) => {
            let path = expand_redirect_word(ctx, w)?;
            let fd = cake_platform::get()
                .open_file(&path, FileOpenMode::Read)
                .map_err(|e| alloc::format!("cake: {path}: {e}"))?;
            *slot_mut(fds, slot) = ChildFd::Fd(fd);
            fds.owned.push(fd);
        }
        (RedirectKind::ReadWrite, RedirectTarget::Word(w)) => {
            let path = expand_redirect_word(ctx, w)?;
            let fd = cake_platform::get()
                .open_file(&path, FileOpenMode::ReadWrite)
                .map_err(|e| alloc::format!("cake: {path}: {e}"))?;
            *slot_mut(fds, slot) = ChildFd::Fd(fd);
            fds.owned.push(fd);
        }
        // `&>file` / `&>>file`: both stdout and stderr.
        (RedirectKind::AndOut, RedirectTarget::Word(w)) => {
            let path = expand_redirect_word(ctx, w)?;
            let fd = cake_platform::get()
                .open_file(&path, FileOpenMode::Write)
                .map_err(|e| alloc::format!("cake: {path}: {e}"))?;
            fds.stdout = ChildFd::Fd(fd);
            fds.stderr = ChildFd::Fd(fd);
            fds.owned.push(fd);
        }
        (RedirectKind::AndAppend, RedirectTarget::Word(w)) => {
            let path = expand_redirect_word(ctx, w)?;
            let fd = cake_platform::get()
                .open_file(&path, FileOpenMode::Append)
                .map_err(|e| alloc::format!("cake: {path}: {e}"))?;
            fds.stdout = ChildFd::Fd(fd);
            fds.stderr = ChildFd::Fd(fd);
            fds.owned.push(fd);
        }
        // `N>&M` / `N<&M`: duplicate an existing fd.
        (RedirectKind::DupOutput, RedirectTarget::Fd(target)) => {
            *slot_mut(fds, slot) = ChildFd::Fd(*target as Fd);
        }
        (RedirectKind::DupInput, RedirectTarget::Fd(target)) => {
            *slot_mut(fds, slot) = ChildFd::Fd(*target as Fd);
        }
        // `>&-` / `<&-`: close.
        (RedirectKind::DupOutput | RedirectKind::DupInput, RedirectTarget::Close) => {
            *slot_mut(fds, slot) = ChildFd::Close;
        }
        // `<<EOF`: pipe the (already collected) body to stdin.
        (RedirectKind::Heredoc, RedirectTarget::Heredoc { body, .. }) => {
            let text = body.borrow().clone().unwrap_or_default();
            setup_stdin_string(&mut fds.stdin, &text)?;
            fds.owned.push(match fds.stdin {
                ChildFd::Fd(fd) => fd,
                _ => unreachable!(),
            });
        }
        // `<<<word`: pipe the expanded word + trailing newline to stdin
        // (bash appends a newline to herestrings).
        (RedirectKind::HereString, RedirectTarget::Word(w)) => {
            let mut text = expand_redirect_word(ctx, w)?;
            text.push('\n');
            setup_stdin_string(&mut fds.stdin, &text)?;
            fds.owned.push(match fds.stdin {
                ChildFd::Fd(fd) => fd,
                _ => unreachable!(),
            });
        }
        // M2c will add process substitution and fd>2 support.
        _ => {
            return Err(alloc::format!(
                "cake: unsupported redirection {:?}",
                r.kind
            ));
        }
    }
    Ok(())
}

/// Create a pipe, write `text`, and set the read end as stdin.
fn setup_stdin_string(stdin: &mut ChildFd, text: &str) -> Result<(), String> {
    let (r, w) = cake_platform::get()
        .pipe(false)
        .map_err(|e| alloc::format!("cake: pipe: {e}"))?;
    let bytes = text.as_bytes();
    let mut written = 0;
    while written < bytes.len() {
        let n = cake_platform::get()
            .write(w, &bytes[written..])
            .map_err(|e| alloc::format!("cake: write: {e}"))?;
        if n == 0 {
            break;
        }
        written += n;
    }
    // Close the write end: downstream reads get EOF once we do.
    cake_platform::get()
        .close(w)
        .map_err(|e| alloc::format!("cake: close: {e}"))?;
    *stdin = ChildFd::Fd(r);
    Ok(())
}
