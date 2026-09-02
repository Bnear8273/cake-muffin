//! Rendering: computing display columns (ANSI-aware), the visible window of a
//! long line (horizontal scroll), and the [`Render`] description the driver
//! turns into terminal bytes.

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;
use unicode_width::UnicodeWidthChar;

/// Display width in terminal columns, ignoring ANSI escape sequences and
/// counting wide (CJK/emoji) characters as 2 columns.
pub fn display_width(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    let mut w = 0;
    while i < b.len() {
        if b[i] == 0x1b {
            i += 1;
            if i < b.len() && b[i] == b'[' {
                // CSI: ESC [ params final (0x40-0x7e)
                i += 1;
                while i < b.len() && !(0x40..=0x7e).contains(&b[i]) {
                    i += 1;
                }
                if i < b.len() {
                    i += 1;
                }
            } else if i < b.len() {
                // Other two-byte escape (ESC O ...): skip the second byte.
                i += 1;
            }
            continue;
        }
        let Some(ch) = core::str::from_utf8(&b[i..])
            .ok()
            .and_then(|s| s.chars().next())
        else {
            i += 1;
            continue;
        };
        w += UnicodeWidthChar::width(ch).unwrap_or(0);
        i += ch.len_utf8();
    }
    w
}

/// The byte range `[start, end)` of a (plain-text) line that fits in
/// `avail` columns with the cursor kept visible.
pub fn visible_window(line: &str, cursor: usize, avail: usize) -> (usize, usize) {
    let total = display_width(line);
    if total <= avail || avail == 0 {
        return (0, line.len());
    }
    let cur_col = display_width(&line[..cursor]);
    let start_col = cur_col.saturating_sub(avail - 1).min(total - 1);
    let start = byte_at_col(line, start_col);
    let end = byte_at_col_from(line, start, avail);
    (start, end)
}

/// Truncate `s` to at most `max_cols` display columns, on a char boundary.
pub fn truncate_to_cols(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let mut acc = 0;
    let mut end = 0;
    for (i, ch) in s.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > max_cols {
            break;
        }
        acc += w;
        end = i + ch.len_utf8();
    }
    s[..end].to_owned()
}

/// Truncate `s` (which may contain ANSI CSI sequences) to at most `max_cols`
/// visible columns. Escape sequences are copied verbatim regardless of the
/// budget, so colour state stays consistent up to the cut point.
pub fn truncate_ansi_to_cols(s: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let b = s.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let mut acc = 0;
    while i < b.len() {
        if b[i] == 0x1b {
            if i + 1 < b.len() && b[i + 1] == b'[' {
                let start = i;
                i += 2;
                while i < b.len() && !(0x40..=0x7e).contains(&b[i]) {
                    i += 1;
                }
                if i < b.len() {
                    i += 1;
                }
                out.push_str(&s[start..i]);
            } else if i + 1 < b.len() {
                out.push_str(&s[i..i + 2]);
                i += 2;
            } else {
                break;
            }
            continue;
        }
        let Some(ch) = core::str::from_utf8(&b[i..])
            .ok()
            .and_then(|s| s.chars().next())
        else {
            i += 1;
            continue;
        };
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > max_cols {
            break;
        }
        acc += w;
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// What the driver needs to paint one frame of the editing line.
#[derive(Debug, Clone)]
pub struct Render {
    /// 1 for a single-line prompt, 2 when the prompt has an info line
    /// (first line) above the input line (second line).
    pub prompt_rows: usize,
    /// Display columns of the first prompt line (info line, before any
    /// right-hand content).
    pub line1_cols: usize,
    /// Right-hand prompt content (ANSI), already truncated to the available
    /// width.
    pub right: String,
    /// Display columns of [`Render::right`].
    pub right_cols: usize,
    /// Display columns of the input-line prompt prefix (e.g. `╰─ `).
    pub prompt_cols: usize,
    /// Display columns of the (possibly scrolled) visible line slice.
    pub line_cols: usize,
    /// Display columns of the hint suffix.
    pub hint_cols: usize,
    /// Absolute display column of the cursor (prompt + in-line offset).
    pub cursor_col: usize,
    /// Prompt text, verbatim (may contain ANSI and a `\n` between the
    /// info line and the input-line prefix).
    pub prompt: String,
    /// Highlighted, visible slice of the line (ANSI applied).
    pub line: String,
    /// Dim ANSI-wrapped hint suffix, already truncated to the remaining space.
    pub hint: String,
    /// Candidate display lines to print below the editing line, when the
    /// second Tab was pressed.
    pub completion: Option<Vec<String>>,
}

/// Compute a frame to paint for the current editor state.
///
/// When `prompt` contains a `\n`, the part before it is the info line (first
/// row, with optional right-hand content) and the part after is the input-line
/// prefix (second row). Without a `\n`, the whole prompt is the input-line
/// prefix (single row).
#[allow(clippy::too_many_arguments)]
pub fn compute_render(
    prompt: &str,
    right: &str,
    buffer: &str,
    cursor: usize,
    hint: Option<&str>,
    completion: Option<&[crate::Candidate]>,
    width: usize,
    highlight: &dyn Fn(&str) -> String,
) -> Render {
    let (line1, line2) = match prompt.split_once('\n') {
        Some((a, b)) => (a, b),
        None => ("", prompt),
    };
    let prompt_rows = if line1.is_empty() { 1 } else { 2 };

    let line1_cols = display_width(line1);
    let right = if line1.is_empty() {
        String::new()
    } else {
        // Leave at least one column for the gap before the right-hand side.
        let avail = width.saturating_sub(line1_cols).saturating_sub(1);
        let mut r = truncate_ansi_to_cols(right, avail);
        // Ensure the truncated right ends with a reset so colours don't
        // bleed into the gap or the next line.
        if !r.is_empty() && !r.ends_with("\x1b[0m") {
            r.push_str("\x1b[0m");
        }
        r
    };
    let right_cols = display_width(&right);

    let prompt_cols = display_width(line2);
    let avail = width.saturating_sub(prompt_cols).max(1);
    let cur_col = display_width(&buffer[..cursor]);

    let (start, end) = visible_window(buffer, cursor, avail);
    let slice = &buffer[start..end];
    let line = highlight(slice);
    let line_cols = display_width(&line);
    let cursor_col = prompt_cols + cur_col.saturating_sub(display_width(&buffer[..start]));

    let remaining = width.saturating_sub(prompt_cols + line_cols);
    let mut hint_out = String::new();
    let mut hint_cols = 0;
    if cursor == buffer.len()
        && let Some(h) = hint
        && remaining > 0
    {
        let t = truncate_to_cols(h, remaining);
        hint_cols = display_width(&t);
        if !t.is_empty() {
            hint_out = alloc::format!("\x1b[2m{t}\x1b[0m");
        }
    }

    let completion_out = completion.map(|cands| cands.iter().map(|c| c.display.clone()).collect());

    Render {
        prompt_rows,
        line1_cols,
        right,
        right_cols,
        prompt_cols,
        line_cols,
        hint_cols,
        cursor_col,
        prompt: prompt.to_owned(),
        line,
        hint: hint_out,
        completion: completion_out,
    }
}

/// Byte offset of the first character whose display column reaches `col`.
fn byte_at_col(line: &str, col: usize) -> usize {
    let mut acc = 0;
    for (i, ch) in line.char_indices() {
        if acc >= col {
            return i;
        }
        acc += UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    line.len()
}

/// Byte offset (from the line start) of the end of a window starting at byte
/// `start` and spanning at most `max_cols` columns.
fn byte_at_col_from(line: &str, start: usize, max_cols: usize) -> usize {
    let mut acc = 0;
    let mut end = start;
    for (i, ch) in line[start..].char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > max_cols {
            break;
        }
        acc += w;
        end = start + i + ch.len_utf8();
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn plain(s: &str) -> String {
        s.to_owned()
    }

    #[test]
    fn display_width_plain() {
        assert_eq!(display_width(""), 0);
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("你好"), 4);
        assert_eq!(display_width("a你b"), 4);
    }

    #[test]
    fn display_width_strips_ansi() {
        assert_eq!(display_width("\x1b[32mabc\x1b[0m"), 3);
        assert_eq!(display_width("\x1b[1;35mif\x1b[0m"), 2);
        assert_eq!(display_width("\x1b[2mhi\x1b[0m"), 2);
    }

    #[test]
    fn visible_window_fits() {
        assert_eq!(visible_window("hello", 2, 10), (0, 5));
    }

    #[test]
    fn visible_window_scrolls() {
        // Cursor at the end of a long line, width 5: window ends at the line.
        let (s, e) = visible_window("abcdefgh", 8, 5);
        assert_eq!(e, 8);
        assert!(s < 8);
        // The visible slice must be at most 5 columns.
        assert!(display_width(&"abcdefgh"[s..e]) <= 5);
    }

    #[test]
    fn visible_window_keeps_cursor_visible() {
        // Width 4, cursor at column 6 of 8: window must include byte 6.
        let (s, e) = visible_window("abcdefgh", 6, 4);
        assert!(
            s <= 6 && e > 6,
            "window {s}..{e} must contain cursor byte 6"
        );
        assert!(display_width(&"abcdefgh"[s..e]) <= 4);
    }

    #[test]
    fn truncate_respects_columns() {
        assert_eq!(truncate_to_cols("hello", 3), "hel");
        assert_eq!(truncate_to_cols("你好", 2), "你");
        assert_eq!(truncate_to_cols("你好", 1), "");
        assert_eq!(truncate_to_cols("abc", 0), "");
        assert_eq!(truncate_to_cols("abc", 10), "abc");
    }

    #[test]
    fn compute_render_fits() {
        let r = compute_render("$ ", "", "ls", 2, None, None, 80, &plain);
        assert_eq!(r.prompt_cols, 2);
        assert_eq!(r.line_cols, 2);
        assert_eq!(r.cursor_col, 4);
        assert_eq!(r.line, "ls");
        assert!(r.completion.is_none());
        assert_eq!(r.prompt_rows, 1);
        assert!(r.right.is_empty());
    }

    #[test]
    fn compute_render_with_hint() {
        let r = compute_render("$ ", "", "ec", 2, Some("ho"), None, 80, &plain);
        assert_eq!(r.hint, "\x1b[2mho\x1b[0m");
        assert_eq!(r.hint_cols, 2);
    }

    #[test]
    fn compute_render_no_hint_mid_line() {
        let r = compute_render("$ ", "", "ec", 1, Some("ho"), None, 80, &plain);
        assert_eq!(r.hint, "");
    }

    #[test]
    fn compute_render_completion_list() {
        let cands = [
            crate::Candidate {
                display: "ls".into(),
                replacement: "ls ".into(),
            },
            crate::Candidate {
                display: "less".into(),
                replacement: "less ".into(),
            },
        ];
        let r = compute_render("$ ", "", "l", 1, None, Some(&cands), 80, &plain);
        let list = r.completion.unwrap();
        assert_eq!(list, vec!["ls", "less"]);
    }

    #[test]
    fn two_line_prompt_split() {
        // "╭─ [info]\n╰─ " → first line is info, second line is input prefix.
        let r = compute_render("╭─ left\x1b[0m\n╰─ ", "", "ls", 2, None, None, 80, &plain);
        assert_eq!(r.prompt_rows, 2);
        assert_eq!(r.line1_cols, display_width("╭─ left\x1b[0m"));
        assert_eq!(r.prompt_cols, display_width("╰─ "));
        assert_eq!(r.cursor_col, r.prompt_cols + 2); // cursor at "ls" end
    }

    #[test]
    fn right_content_width_and_truncation() {
        // Narrow terminal: right is truncated to width - line1 - 1.
        let r = compute_render(
            "╭─ x\x1b[0m\n╰─ ",
            "\x1b[0mRIGHTMORE\x1b[0m",
            "",
            0,
            None,
            None,
            10,
            &plain,
        );
        assert_eq!(r.prompt_rows, 2);
        assert_eq!(r.line1_cols, 4); // "╭─ x"
        // avail for right = 10 - 4 - 1 = 5 → "RIGHT" (5 cols).
        assert_eq!(r.right_cols, 5);
        assert!(r.right_cols < 10 - r.line1_cols);
        // The truncated right still ends with a reset.
        assert!(r.right.ends_with("\x1b[0m"));
    }

    #[test]
    fn truncate_ansi_counts_visible_only() {
        let s = "\x1b[38;5;70m✔ \x1b[0m\x1b[38;5;244m╱ 5s\x1b[0m";
        assert_eq!(display_width(&truncate_ansi_to_cols(s, 5)), 5);
        assert_eq!(
            display_width(&truncate_ansi_to_cols(s, 100)),
            display_width(s)
        );
        // ANSI sequences survive the cut.
        assert!(truncate_ansi_to_cols(s, 5).starts_with("\x1b[38;5;70m"));
    }

    #[test]
    fn right_ignored_for_single_line() {
        let r = compute_render("$ ", "RIGHT", "ls", 2, None, None, 80, &plain);
        assert_eq!(r.prompt_rows, 1);
        assert!(r.right.is_empty());
        assert_eq!(r.right_cols, 0);
    }

    #[test]
    fn newline_split_keeps_second_line_for_input() {
        // Cursor math is relative to the second (input) line, not the info line.
        let r = compute_render("╭─ info\n> ", "", "ab", 1, None, None, 80, &plain);
        assert_eq!(r.prompt_rows, 2);
        assert_eq!(r.prompt_cols, 2); // "> "
        assert_eq!(r.cursor_col, 3); // after 'a'
    }
}
