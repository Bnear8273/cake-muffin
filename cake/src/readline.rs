//! The platform side of the line editor: raw-mode entry/exit and the
//! read-paint loop that drives `cake_editor::Editor`. All byte I/O goes
//! through the global `Platform` so the logic stays portable; only this thin
//! driver touches terminal state.

use cake_editor::{Candidate, EditEvent, Editor, Key, KeyParser, Render, Services};
use cake_platform::{Platform, PlatformError, Termios};

/// Restore cooked terminal mode when dropped (RAII).
struct RawModeGuard {
    platform: &'static dyn Platform,
    saved: Termios,
}

impl RawModeGuard {
    fn enter() -> Result<Self, PlatformError> {
        let p = cake_platform::get();
        // "Get twice": each call returns an independent copy, so `saved`
        // keeps the cooked state while `raw` is mutated (Termios: !Clone).
        let saved = p.get_termios(0)?;
        let mut raw = p.get_termios(0)?;
        raw.set_raw();
        p.set_termios(0, &raw)?;
        Ok(RawModeGuard { platform: p, saved })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = self.platform.set_termios(0, &self.saved);
    }
}

/// What one `read_loop` call returned.
pub(crate) enum ReadOutcome {
    Line(String),
    Eof,
    Interrupted,
}

/// Run one interactive line-edit session. Enters raw mode for the duration
/// (restored before returning), paints the prompt/line as the user types, and
/// returns the outcome. The `parser` and `pending` buffers persist across
/// calls so that a batch of keys that crosses a line boundary (e.g. pasted
/// multi-line input) is not lost: keys after the returning event stay in
/// `pending` for the next session.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn read_loop<'a>(
    editor: &mut Editor,
    parser: &mut KeyParser,
    pending: &mut Vec<Key>,
    prompt: &'a str,
    right: &'a str,
    highlight: &'a dyn Fn(&str) -> String,
    hint: &'a dyn Fn(&str, &[String]) -> Option<String>,
    complete: &'a dyn Fn(&str, usize) -> Option<(usize, Vec<Candidate>)>,
) -> Result<ReadOutcome, String> {
    let platform = cake_platform::get();
    let _guard = RawModeGuard::enter().map_err(|e| format!("cake: {e}"))?;
    let cols = platform
        .terminal_size(0)
        .map(|t| t.cols as usize)
        .ok()
        .filter(|&c| c > 0)
        .unwrap_or(80);
    let srv = Services {
        prompt,
        right,
        width: cols,
        highlight,
        hint,
        complete,
    };

    editor.reset();
    let mut buf = [0u8; 64];
    // How many prompt rows the previous frame painted (0 = none yet).
    let mut prev_rows = 0usize;

    // Paint the fresh prompt before reading anything.
    prev_rows = paint(&editor.render(&srv), cols, prev_rows)?;

    loop {
        if pending.is_empty() {
            let n = platform
                .read(0, &mut buf)
                .map_err(|e| format!("cake: {e}"))?;
            if n == 0 {
                return Ok(ReadOutcome::Eof);
            }
            parser.feed(&buf[..n], pending);
            if pending.is_empty() {
                // Only partial sequences arrived; wait for more input.
                continue;
            }
        }

        // Process every key in the batch; the final state is painted once,
        // so a large paste repaints only a handful of times.
        let mut repaint = false;
        while let Some(k) = pending.first().copied() {
            pending.remove(0);
            match editor.handle(k, &srv) {
                EditEvent::Redraw => repaint = true,
                EditEvent::NoChange => {}
                EditEvent::Submit(line) => {
                    // p10k transient prompt: collapse the just-submitted
                    // two-row prompt into a single `> <command>` line. The
                    // command keeps its syntax highlight (green for a found
                    // command) exactly as it appeared while editing.
                    let mut out = Vec::with_capacity(64);
                    let hl = (srv.highlight)(&line);
                    transient_collapse(&mut out, &hl, prev_rows, cols);
                    let platform = cake_platform::get();
                    let _ = platform.write(1, &out).map_err(|e| format!("cake: {e}"))?;
                    return Ok(ReadOutcome::Line(line));
                }
                EditEvent::Eof => {
                    write_out("\r\n")?;
                    return Ok(ReadOutcome::Eof);
                }
                EditEvent::Interrupted => {
                    write_out("\r\n")?;
                    return Ok(ReadOutcome::Interrupted);
                }
            }
        }

        if repaint {
            if editor.take_clear_screen() {
                write_out("\x1b[2J\x1b[H")?;
                prev_rows = 0;
            }
            prev_rows = paint(&editor.render(&srv), cols, prev_rows)?;
        }
    }
}

/// Paint one frame. With a two-row prompt the info line (first row) is
/// rewritten with the right-hand content and gap fill, then the input line
/// (second row) is rewritten. Returns the number of prompt rows painted, so
/// the caller can reposition the cursor correctly on the next frame.
fn paint(r: &Render, cols: usize, prev_rows: usize) -> Result<usize, String> {
    let mut out = Vec::with_capacity(128);
    if r.prompt_rows == 2 {
        let idx = r.prompt.find('\n').expect("two-row prompt contains \\n");
        // Move back up to the info line (on the first frame the terminal is
        // already positioned for a fresh prompt, so nothing is needed).
        if prev_rows == 2 {
            out.extend_from_slice(b"\r\x1b[1A");
        } else {
            out.push(b'\r');
        }
        out.extend_from_slice(&r.prompt.as_bytes()[..idx]);
        // Gap fill between the info line and the right-hand content.
        let gap = if r.right.is_empty() {
            cols.saturating_sub(r.line1_cols)
        } else {
            cols.saturating_sub(r.line1_cols + r.right_cols)
        };
        if gap > 0 {
            out.extend_from_slice(b"\x1b[38;5;240m");
            out.extend_from_slice("─".repeat(gap).as_bytes());
            out.extend_from_slice(b"\x1b[0m");
        }
        if !r.right.is_empty() {
            out.extend_from_slice(r.right.as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&r.prompt.as_bytes()[idx + 1..]);
        out.extend_from_slice(r.line.as_bytes());
        out.extend_from_slice(r.hint.as_bytes());
    } else {
        out.push(b'\r');
        out.extend_from_slice(r.prompt.as_bytes());
        out.extend_from_slice(r.line.as_bytes());
        out.extend_from_slice(r.hint.as_bytes());
    }
    // Clear from the end of the written text to the bottom of the screen,
    // erasing any stale completion list from a previous frame.
    out.extend_from_slice(b"\x1b[J");
    let end_col = r.prompt_cols + r.line_cols + r.hint_cols;
    if end_col > r.cursor_col {
        let back = end_col - r.cursor_col;
        out.extend_from_slice(format!("\x1b[{back}D").as_bytes());
    }
    if let Some(list) = &r.completion {
        for item in list {
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(item.as_bytes());
        }
        // Return to the editing line and re-place the cursor.
        out.extend_from_slice(format!("\x1b[{}A\r", list.len()).as_bytes());
        if r.cursor_col > 0 {
            out.extend_from_slice(format!("\x1b[{}C", r.cursor_col).as_bytes());
        }
    }
    let platform = cake_platform::get();
    let _ = platform.write(1, &out).map_err(|e| format!("cake: {e}"))?;
    Ok(r.prompt_rows)
}

/// Collapse the just-submitted two-row prompt into a single `> <line>`
/// (p10k transient prompt), so executed lines don't leave two rows behind.
/// `line` is already syntax-highlighted (ANSI). Single-row (legacy) prompts
/// are left untouched: just a newline.
fn transient_collapse(out: &mut Vec<u8>, line: &str, prev_rows: usize, cols: usize) {
    if prev_rows == 2 {
        // Up to the info line, erase both prompt rows (+ any completion list),
        // then draw `> <command>`.
        out.extend_from_slice(b"\r\x1b[1A\x1b[J");
        // Transient prompt symbol in the same frame colour as `╰─`.
        out.extend_from_slice(b"\x1b[38;5;240m> \x1b[0m");
        let t = cake_editor::render::truncate_ansi_to_cols(line, cols.saturating_sub(2));
        out.extend_from_slice(t.as_bytes());
    }
    out.extend_from_slice(b"\r\n");
}

fn write_out(s: &str) -> Result<(), String> {
    let platform = cake_platform::get();
    let _ = platform
        .write(1, s.as_bytes())
        .map_err(|e| format!("cake: {e}"))?;
    Ok(())
}
