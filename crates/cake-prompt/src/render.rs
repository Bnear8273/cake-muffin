//! Render the facts into an ANSI colour-segmented, two-line prompt.

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;
use unicode_width::UnicodeWidthChar;

use crate::config::{Color, Config, Segment};
use crate::git::GitStatus;

/// The two-line prompt structure returned by [`render`].
///
/// The driver builds the editor prompt as `format!("{}\n{}", left, input)`
/// and passes `right` to the editor's right-prompt slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// First line: `╭─ ` prefix + left segments + tail arrow (ANSI).
    pub left: String,
    /// Right-hand content (ANSI): start arrow + right segments.
    pub right: String,
    /// Input-line prefix (ANSI): `╰─ `.
    pub input: String,
}

/// Facts about the current shell state, gathered by the driver.
#[derive(Debug)]
pub struct Facts<'a> {
    /// The username (`$USER`), or `None` if unavailable.
    pub user: Option<&'a str>,
    /// The current directory, already `~`-abbreviated.
    pub dir: &'a str,
    /// Working-tree status, or `None` outside a git repository.
    pub git: Option<GitStatus>,
    /// The exit status of the last command (`last_status.status_code()`).
    pub exit: i32,
    /// Elapsed wall time of the last command (ms); `None` on the first prompt.
    pub elapsed_ms: Option<u64>,
    /// Local clock string (`HH:MM:SS`), or `None` if unavailable.
    pub clock: Option<&'a str>,
}

/// Header/input-line frame symbols (p10k rainbow theme): `╭─` and `╰─` in
/// colour 240.
const FRAME_FG: u8 = 240;
const HEADER: &str = "\u{256d}\u{2500}"; // ╭─
const INPUT: &str = "\u{2570}\u{2500}"; // ╰─
/// Gap-fill colour between left and right groups.
const GAP_FG: u8 = 244;
/// Path component separator between `render_dir` parts.
const SUBSEP: &str = "/";
/// Segment separator (different background colours): left curve.
const SEP_LEFT: char = '\u{e0bc}';
/// Left tail and right start arrows.
const TAIL_LEFT: char = '\u{e0b0}';
const START_RIGHT: char = '\u{e0b2}';

/// Git sub-segment colours (on the shared git background).
const GIT_BRANCH_FG: u8 = 46; // green
const GIT_STAGED_FG: u8 = 220; // yellow (+N)
const GIT_UNSTAGED_FG: u8 = 208; // orange (!N)
const GIT_UNTRACKED_FG: u8 = 39; // blue (?N)
const GIT_AHEAD_FG: u8 = 46; // green (↑N)
const GIT_BEHIND_FG: u8 = 196; // red (↓N)

/// Render the segmented prompt. When no segment of either group applies, all
/// fields of the returned [`Rendered`] are empty (the driver falls back).
pub fn render(facts: &Facts, cfg: &Config) -> Rendered {
    let left = render_group(facts, cfg, &cfg.left_segments, true);
    let right = render_group(facts, cfg, &cfg.right_segments, false);
    if left.is_empty() && right.is_empty() {
        return Rendered {
            left: String::new(),
            right: String::new(),
            input: String::new(),
        };
    }
    Rendered {
        left: alloc::format!("\x1b[38;5;{FRAME_FG}m{HEADER}\x1b[0m{left}",),
        right,
        input: alloc::format!("\x1b[38;5;{FRAME_FG}m{INPUT}\x1b[0m "),
    }
}

/// Render one group (left or right) of segments into fully-coloured text.
fn render_group(facts: &Facts, cfg: &Config, segments: &[Segment], is_left: bool) -> String {
    let mut segs: Vec<(String, Color)> = Vec::new();
    for &seg in segments {
        if let Some((text, color)) = segment(facts, cfg, seg) {
            segs.push((text, color));
        }
    }
    if segs.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for (i, (text, color)) in segs.iter().enumerate() {
        if i > 0 && is_left {
            // Left segments are joined by the curve (visible because each
            // segment has a distinct background). Right segments are joined
            // by nothing — each is padded with coloured spaces instead.
            let prev_bg = segs[i - 1].1.bg;
            out.push_str(&alloc::format!(
                "\x1b[38;5;{prev_bg};48;5;{}m{SEP_LEFT}",
                color.bg
            ));
        }
        out.push_str(text);
    }

    if is_left {
        // Tail arrow in the last segment's background colour, on default bg.
        // Every segment already ends with a trailing space, so the arrow sits
        // right after it (no extra separator needed).
        let last_bg = segs[segs.len() - 1].1.bg;
        out.push_str(&alloc::format!(
            "\x1b[49m\x1b[38;5;{last_bg}m{TAIL_LEFT}\x1b[0m"
        ));
    } else {
        // Start arrow in the first segment's background colour, on default bg.
        let first_bg = segs[0].1.bg;
        out.insert_str(0, &alloc::format!("\x1b[38;5;{first_bg}m{START_RIGHT}"));
        out.push_str("\x1b[0m");
    }
    out
}

/// The coloured text and background colour of one segment.
fn segment(facts: &Facts, cfg: &Config, seg: Segment) -> Option<(String, Color)> {
    match seg {
        Segment::User => {
            let user = facts.user?;
            if user.is_empty() {
                return None;
            }
            let c = cfg.color(Segment::User);
            Some((styled(user, c), c))
        }
        Segment::Dir => {
            let t = truncate_dir(facts.dir, cfg.maxlen);
            let c = cfg.color(Segment::Dir);
            let inner = render_dir(&t, c, cfg.dir_alt_fg);
            // Pad with a coloured space on each side (like styled).
            let text = alloc::format!("\x1b[38;5;{};48;5;{}m {inner} ", c.fg, c.bg);
            Some((text, c))
        }
        Segment::Git => {
            let git = facts.git.as_ref()?;
            let bg = cfg.color(Segment::Git).bg;
            Some((git_colored(git, bg), Color::new(bg, GIT_BRANCH_FG)))
        }
        Segment::Status => {
            let (text, c) = if facts.exit == 0 {
                ("\u{2713}".to_owned(), cfg.color(Segment::Status)) // ✓
            } else {
                (alloc::format!("\u{2718} {}", facts.exit), cfg.status_fail()) // ✘ N
            };
            Some((styled(&text, c), c))
        }
        Segment::Time => {
            let ms = facts.elapsed_ms?;
            if ms < cfg.time_ms {
                return None;
            }
            let c = cfg.color(Segment::Time);
            Some((styled(&format_elapsed(ms), c), c))
        }
        Segment::Clock => {
            if !cfg.clock_enabled {
                return None;
            }
            let clock = facts.clock?;
            if clock.is_empty() {
                return None;
            }
            let c = cfg.color(Segment::Clock);
            Some((styled(clock, c), c))
        }
    }
}

/// Colour `text` with `fg`/`bg`. The surrounding spaces are on the segment
/// background so the segment is padded on both sides.
fn styled(text: &str, c: Color) -> String {
    alloc::format!("\x1b[38;5;{};48;5;{}m {text} ", c.fg, c.bg)
}

/// Split a `~`-abbreviated or absolute path into coloured components joined
/// by `/`; the foreground alternates between the segment colour and `alt_fg`.
fn render_dir(dir: &str, color: Color, alt_fg: u8) -> String {
    let mut parts: Vec<&str> = dir.split('/').collect();
    let mut start = 0;
    if parts.first() == Some(&"") {
        // Absolute path: fold the leading "/" into the first component.
        if parts.len() > 1 {
            let first_len = parts[1].len();
            parts[1] = &dir[..first_len + 1];
        }
        start = 1;
    }
    let mut out = String::new();
    let mut idx = 0usize;
    for part in parts.iter().skip(start) {
        if part.is_empty() {
            continue;
        }
        if idx > 0 {
            out.push_str(&alloc::format!("\x1b[38;5;{GAP_FG}m{SUBSEP}"));
        }
        let fg = if idx.is_multiple_of(2) {
            color.fg
        } else {
            alt_fg
        };
        out.push_str(&alloc::format!("\x1b[38;5;{fg};48;5;{}m{part}", color.bg));
        idx += 1;
    }
    out
}

/// Render the git segment with coloured sub-segments:
/// ` branch !N ?N +N ↑N ↓N` on a shared background, space-separated.
fn git_colored(git: &GitStatus, bg: u8) -> String {
    let mut s = String::new();
    let branch = match &git.branch {
        Some(b) => b.clone(),
        None => "HEAD".into(), // detached
    };
    s.push_str(&alloc::format!(
        "\x1b[38;5;{GIT_BRANCH_FG};48;5;{bg}m {branch}"
    ));
    if git.staged > 0 {
        s.push_str(&alloc::format!(
            "\x1b[38;5;{GIT_STAGED_FG};48;5;{bg}m +{}",
            git.staged
        ));
    }
    if git.unstaged > 0 {
        s.push_str(&alloc::format!(
            "\x1b[38;5;{GIT_UNSTAGED_FG};48;5;{bg}m !{}",
            git.unstaged
        ));
    }
    if git.untracked > 0 {
        s.push_str(&alloc::format!(
            "\x1b[38;5;{GIT_UNTRACKED_FG};48;5;{bg}m ?{}",
            git.untracked
        ));
    }
    if git.ahead > 0 {
        s.push_str(&alloc::format!(
            "\x1b[38;5;{GIT_AHEAD_FG};48;5;{bg}m \u{2191}{}",
            git.ahead
        ));
    }
    if git.behind > 0 {
        s.push_str(&alloc::format!(
            "\x1b[38;5;{GIT_BEHIND_FG};48;5;{bg}m \u{2193}{}",
            git.behind
        ));
    }
    s.push(' ');
    s
}

/// `500ms`, `1.2s`, or `1m 5s`.
fn format_elapsed(ms: u64) -> String {
    if ms < 1000 {
        alloc::format!("{ms}ms")
    } else if ms < 60_000 {
        alloc::format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let total_s = ms / 1000;
        alloc::format!("{}m {}s", total_s / 60, total_s % 60)
    }
}

/// Truncate a `~`-abbreviated path to at most `max` display columns, keeping
/// the head (through the first slash) and the last two components, joined by
/// `…`. `max == 0` disables truncation.
fn truncate_dir(dir: &str, max: usize) -> String {
    if max == 0 || width(dir) <= max {
        return dir.to_owned();
    }
    let head_end = dir.find('/').map(|i| i + 1).unwrap_or(0);
    let head = &dir[..head_end];
    let tail_start = dir
        .rmatch_indices('/')
        .nth(1)
        .map(|(i, _)| i + 1)
        .unwrap_or(head_end);
    let tail = &dir[tail_start..];

    let mut out = alloc::format!("{head}\u{2026}"); // "…"
    let mut w = width(&out);
    if w > max {
        out.clear();
        out.push('\u{2026}');
        w = 1;
    }
    for ch in tail.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

fn width(s: &str) -> usize {
    s.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, TIME_DEFAULT_MS};

    fn cfg(left: &[Segment], right: &[Segment]) -> Config {
        Config {
            left_segments: left.to_vec(),
            right_segments: right.to_vec(),
            colors: alloc::collections::BTreeMap::new(),
            status_fail: Color::new(52, 196),
            git_dirty: Color::new(236, 229),
            time_ms: TIME_DEFAULT_MS,
            maxlen: 0,
            dir_alt_fg: 39,
            clock_enabled: true,
        }
    }

    fn facts<'a>(
        user: Option<&'a str>,
        dir: &'a str,
        git: Option<GitStatus>,
        exit: i32,
        ms: Option<u64>,
        clock: Option<&'a str>,
    ) -> Facts<'a> {
        Facts {
            user,
            dir,
            git,
            exit,
            elapsed_ms: ms,
            clock,
        }
    }

    fn git(branch: Option<&str>, staged: usize, unstaged: usize, untracked: usize) -> GitStatus {
        GitStatus {
            branch: branch.map(str::to_owned),
            staged,
            unstaged,
            untracked,
            ahead: 0,
            behind: 0,
        }
    }

    #[test]
    fn empty_groups_render_empty() {
        let r = render(&facts(None, "~", None, 0, None, None), &cfg(&[], &[]));
        assert!(r.left.is_empty());
        assert!(r.right.is_empty());
        assert!(r.input.is_empty());
    }

    #[test]
    fn left_group_header_and_tail() {
        let c = cfg(&[Segment::User], &[]);
        let r = render(&facts(Some("bnear"), "~", None, 0, None, None), &c);
        assert!(r.left.starts_with("\x1b[38;5;240m\u{256d}\u{2500}\x1b[0m"));
        assert!(r.left.contains("bnear"));
        // Tail arrow in the last segment's bg (237), on default bg.
        assert!(r.left.ends_with("\x1b[49m\x1b[38;5;238m\u{e0b0}\x1b[0m"));
        assert_eq!(r.input, "\x1b[38;5;240m\u{2570}\u{2500}\x1b[0m ");
        assert!(r.right.is_empty());
    }

    #[test]
    fn segment_separators_all_curves() {
        // user → dir → git: every segment boundary uses the left curve,
        // regardless of background colour; `╱` only splits dir path parts.
        let c = cfg(&[Segment::User, Segment::Dir, Segment::Git], &[]);
        let f = facts(
            Some("u"),
            "~/a",
            Some(git(Some("main"), 0, 0, 0)),
            0,
            None,
            None,
        );
        let out = render(&f, &c).left;
        assert_eq!(out.matches('\u{e0bc}').count(), 2); // user→dir, dir→git
        assert_eq!(out.matches('/').count(), 1); // inside "~/a"
    }

    #[test]
    fn right_group_start_arrow_and_reset() {
        let c = cfg(&[], &[Segment::Status]);
        let r = render(&facts(None, "~", None, 0, None, None), &c);
        // E0B2 in the first right segment's bg (234 = shared right bg).
        assert!(r.right.starts_with("\x1b[38;5;234m\u{e0b2}"));
        assert!(r.right.contains("\u{2713}")); // ✓
        assert!(r.right.ends_with("\x1b[0m"));
    }

    #[test]
    fn dir_path_split_alternates_colors() {
        let c = cfg(&[Segment::Dir], &[]);
        let f = facts(None, "~/Projects/cake-shell", None, 0, None, None);
        let out = render(&f, &c).left;
        // Components joined by /, alternating fg 31/39 on bg 236.
        assert!(out.contains(
            "\x1b[38;5;39;48;5;236mProjects\x1b[38;5;244m/\x1b[38;5;31;48;5;236mcake-shell"
        ));
    }

    #[test]
    fn absolute_path_keeps_root_slash() {
        let c = cfg(&[Segment::Dir], &[]);
        let f = facts(None, "/usr/local/bin", None, 0, None, None);
        let out = render(&f, &c).left;
        assert!(out.contains(
            "\x1b[38;5;244m/\x1b[38;5;39;48;5;236mlocal\x1b[38;5;244m/\x1b[38;5;31;48;5;236mbin"
        ));
    }

    #[test]
    fn status_success_and_failure() {
        let c = cfg(&[], &[Segment::Status]);
        let r = render(&facts(None, "~", None, 0, None, None), &c);
        assert!(r.right.contains("\x1b[38;5;46;48;5;234m \u{2713} "));
        let r = render(&facts(None, "~", None, 127, None, None), &c);
        assert!(r.right.contains("\x1b[38;5;196;48;5;52m \u{2718} 127 "));
    }

    #[test]
    fn git_colored_subsegments() {
        let c = cfg(&[Segment::Git], &[]);
        // Clean: branch in green, no dirty markers.
        let f = facts(None, "~", Some(git(Some("main"), 0, 0, 0)), 0, None, None);
        let out = render(&f, &c).left;
        assert!(out.contains("\x1b[38;5;46;48;5;234m main"));
        // Dirty: +yellow staged, !orange unstaged, ?blue untracked.
        let f = facts(None, "~", Some(git(Some("main"), 1, 2, 3)), 0, None, None);
        let out = render(&f, &c).left;
        assert!(out.contains("\x1b[38;5;220;48;5;234m +1"));
        assert!(out.contains("\x1b[38;5;208;48;5;234m !2"));
        assert!(out.contains("\x1b[38;5;39;48;5;234m ?3"));
    }

    #[test]
    fn clock_segment() {
        let c = cfg(&[], &[Segment::Clock]);
        let f = facts(None, "~", None, 0, None, Some("17:51:22"));
        assert!(render(&f, &c).right.contains("17:51:22"));
        // No clock when unavailable.
        let f = facts(None, "~", None, 0, None, None);
        assert!(render(&f, &c).right.is_empty());
    }

    #[test]
    fn clock_disabled() {
        let mut c = cfg(&[], &[Segment::Clock]);
        c.clock_enabled = false;
        let f = facts(None, "~", None, 0, None, Some("17:51:22"));
        assert!(render(&f, &c).right.is_empty());
    }

    #[test]
    fn time_only_over_threshold() {
        let c = cfg(&[], &[Segment::Time]);
        let f = facts(None, "~", None, 0, Some(500), None);
        assert!(render(&f, &c).right.is_empty());
        let f = facts(None, "~", None, 0, Some(1200), None);
        assert!(render(&f, &c).right.contains("1.2s"));
    }

    #[test]
    fn elapsed_formats() {
        assert_eq!(format_elapsed(500), "500ms");
        assert_eq!(format_elapsed(2000), "2.0s");
        assert_eq!(format_elapsed(1500), "1.5s");
        assert_eq!(format_elapsed(65_000), "1m 5s");
    }

    #[test]
    fn dir_truncation() {
        assert_eq!(truncate_dir("~/a/b/c/d", 0), "~/a/b/c/d");
        assert_eq!(truncate_dir("~/a/b", 30), "~/a/b");
        let t = truncate_dir("~/Projects/cake-shell/src", 12);
        assert!(t.contains('\u{2026}'));
        assert!(t.starts_with("~/"));
        assert!(width(&t) <= 12);
        assert!(truncate_dir("/usr/local/share/doc", 8).starts_with('/'));
    }
}
