//! Configuration for the segmented prompt, parsed from `CAKE_PROMPT_*`
//! environment variables (typically exported in the rc file).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Default command-execution time threshold (ms): elapsed times below this
/// do not render a time segment.
pub const TIME_DEFAULT_MS: u64 = 1000;

/// A prompt segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Segment {
    User,
    Dir,
    Git,
    Status,
    Time,
    Clock,
}

impl Segment {
    pub fn name(self) -> &'static str {
        match self {
            Segment::User => "user",
            Segment::Dir => "dir",
            Segment::Git => "git",
            Segment::Status => "status",
            Segment::Time => "time",
            Segment::Clock => "clock",
        }
    }

    fn from_name(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Segment::User),
            "dir" => Some(Segment::Dir),
            "git" => Some(Segment::Git),
            "status" => Some(Segment::Status),
            "time" => Some(Segment::Time),
            "clock" => Some(Segment::Clock),
            _ => None,
        }
    }
}

/// Foreground/background colour pair (0-255 terminal palette).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub bg: u8,
    pub fg: u8,
}

impl Color {
    pub const fn new(bg: u8, fg: u8) -> Self {
        Color { bg, fg }
    }
}

/// The default palette: every segment has a distinct background so the curve
/// separators (coloured with the previous segment's background) stay visible.
pub fn default_color(seg: Segment) -> Color {
    match seg {
        Segment::User => Color::new(238, 46), // green on light grey
        Segment::Dir => Color::new(236, 31),  // red/blue path on grey
        Segment::Git => Color::new(234, 46),  // green branch on dark grey
        // Right-hand segments share the git background so the right side
        // reads as one block matching the git segment.
        Segment::Status => Color::new(234, 46), // green on dark grey
        Segment::Time => Color::new(234, 250),
        Segment::Clock => Color::new(234, 250),
    }
}

/// Prompt configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Left-group segments, in display order. Empty when explicitly disabled.
    pub left_segments: Vec<Segment>,
    /// Right-group segments, in display order. Empty when disabled.
    pub right_segments: Vec<Segment>,
    pub(crate) colors: BTreeMap<Segment, Color>,
    /// Colour for the status segment when the last command failed.
    pub(crate) status_fail: Color,
    /// Colour for the git segment when the repo is dirty.
    pub(crate) git_dirty: Color,
    /// Time threshold (ms).
    pub time_ms: u64,
    /// Maximum display columns for the dir segment (0 = no truncation).
    pub maxlen: usize,
    /// Alternate foreground for the split dir path components.
    pub dir_alt_fg: u8,
    /// Whether the clock segment renders.
    pub clock_enabled: bool,
}

/// The default segment groups when the corresponding env var is unset.
const DEFAULT_LEFT: &[Segment] = &[Segment::User, Segment::Dir, Segment::Git];
const DEFAULT_RIGHT: &[Segment] = &[Segment::Status, Segment::Time, Segment::Clock];

impl Config {
    /// Parse configuration from a variable getter (the driver reads from the
    /// shell's `EnvStack`).
    ///
    /// `CAKE_PROMPT_LEFT` / `CAKE_PROMPT_RIGHT` are comma lists. An explicitly
    /// empty `CAKE_PROMPT_LEFT` disables the whole segmented prompt (the driver
    /// falls back to the legacy prompt); unset values use the defaults.
    pub fn from_env(get: &dyn Fn(&str) -> Option<String>) -> Self {
        let left_segments = parse_list(get("CAKE_PROMPT_LEFT"), DEFAULT_LEFT);
        let right_segments = parse_list(get("CAKE_PROMPT_RIGHT"), DEFAULT_RIGHT);

        let mut colors = BTreeMap::new();
        for seg in [
            Segment::User,
            Segment::Dir,
            Segment::Git,
            Segment::Status,
            Segment::Time,
            Segment::Clock,
        ] {
            let var = alloc::format!("CAKE_PROMPT_COLOR_{}", seg.name().to_uppercase());
            if let Some(c) = get(&var).and_then(|s| parse_color(&s)) {
                colors.insert(seg, c);
            }
        }

        Config {
            left_segments,
            right_segments,
            colors,
            status_fail: get("CAKE_PROMPT_COLOR_FAIL")
                .and_then(|s| parse_color(&s))
                .unwrap_or(Color::new(52, 196)),
            git_dirty: get("CAKE_PROMPT_COLOR_GIT_DIRTY")
                .and_then(|s| parse_color(&s))
                .unwrap_or(Color::new(236, 229)),
            time_ms: get("CAKE_PROMPT_TIME_MS")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(TIME_DEFAULT_MS),
            maxlen: get("CAKE_PROMPT_MAXLEN")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0),
            dir_alt_fg: get("CAKE_PROMPT_DIR_ALT")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(39),
            clock_enabled: !matches!(
                get("CAKE_PROMPT_CLOCK").as_deref().map(str::trim),
                Some("0") | Some("off") | Some("false")
            ),
        }
    }

    pub fn color(&self, seg: Segment) -> Color {
        self.colors
            .get(&seg)
            .copied()
            .unwrap_or_else(|| default_color(seg))
    }

    pub fn status_fail(&self) -> Color {
        self.status_fail
    }

    pub fn git_dirty(&self) -> Color {
        self.git_dirty
    }
}

fn parse_list(raw: Option<String>, default: &[Segment]) -> Vec<Segment> {
    match raw {
        Some(v) if v.trim().is_empty() => Vec::new(),
        Some(v) => v
            .split(',')
            .map(str::trim)
            .filter_map(Segment::from_name)
            .collect(),
        None => default.to_vec(),
    }
}

fn parse_color(s: &str) -> Option<Color> {
    let (bg, fg) = s.split_once(',')?;
    let bg: u8 = bg.trim().parse().ok()?;
    let fg: u8 = fg.trim().parse().ok()?;
    Some(Color { bg, fg })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::borrow::ToOwned;
    use alloc::collections::BTreeMap;
    use alloc::vec;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_owned(), (*v).to_owned());
        }
        m
    }

    fn cfg(pairs: &[(&str, &str)]) -> Config {
        let m = vars(pairs);
        Config::from_env(&|name| m.get(name).cloned())
    }

    #[test]
    fn unset_left_defaults_on() {
        let c = cfg(&[]);
        assert_eq!(
            c.left_segments,
            vec![Segment::User, Segment::Dir, Segment::Git]
        );
        assert_eq!(
            c.right_segments,
            vec![Segment::Status, Segment::Time, Segment::Clock]
        );
    }

    #[test]
    fn explicit_empty_left_disables() {
        assert!(cfg(&[("CAKE_PROMPT_LEFT", "")]).left_segments.is_empty());
        assert!(cfg(&[("CAKE_PROMPT_LEFT", "  ")]).left_segments.is_empty());
        assert!(cfg(&[("CAKE_PROMPT_LEFT", ",")]).left_segments.is_empty());
    }

    #[test]
    fn parses_left_and_right_lists() {
        let c = cfg(&[
            ("CAKE_PROMPT_LEFT", "dir,git"),
            ("CAKE_PROMPT_RIGHT", "status,clock"),
        ]);
        assert_eq!(c.left_segments, vec![Segment::Dir, Segment::Git]);
        assert_eq!(c.right_segments, vec![Segment::Status, Segment::Clock]);
    }

    #[test]
    fn trims_and_drops_unknown() {
        let c = cfg(&[("CAKE_PROMPT_LEFT", " user ,bogus,git ")]);
        assert_eq!(c.left_segments, vec![Segment::User, Segment::Git]);
    }

    #[test]
    fn empty_right_means_no_right_group() {
        let c = cfg(&[("CAKE_PROMPT_RIGHT", "")]);
        assert!(c.right_segments.is_empty());
        assert_eq!(c.left_segments, DEFAULT_LEFT.to_vec());
    }

    #[test]
    fn time_maxlen_and_dir_alt() {
        let c = cfg(&[
            ("CAKE_PROMPT_TIME_MS", "500"),
            ("CAKE_PROMPT_MAXLEN", "30"),
            ("CAKE_PROMPT_DIR_ALT", "31"),
        ]);
        assert_eq!(c.time_ms, 500);
        assert_eq!(c.maxlen, 30);
        assert_eq!(c.dir_alt_fg, 31);
        let c = cfg(&[
            ("CAKE_PROMPT_TIME_MS", "abc"),
            ("CAKE_PROMPT_MAXLEN", "-1"),
            ("CAKE_PROMPT_DIR_ALT", "x"),
        ]);
        assert_eq!(c.time_ms, TIME_DEFAULT_MS);
        assert_eq!(c.maxlen, 0);
        assert_eq!(c.dir_alt_fg, 39);
    }

    #[test]
    fn clock_toggle() {
        assert!(cfg(&[]).clock_enabled);
        assert!(!cfg(&[("CAKE_PROMPT_CLOCK", "0")]).clock_enabled);
        assert!(!cfg(&[("CAKE_PROMPT_CLOCK", "off")]).clock_enabled);
        assert!(cfg(&[("CAKE_PROMPT_CLOCK", "1")]).clock_enabled);
    }

    #[test]
    fn color_override_and_defaults() {
        let c = cfg(&[("CAKE_PROMPT_COLOR_DIR", "236,231")]);
        assert_eq!(c.color(Segment::Dir), Color::new(236, 231));
        assert_eq!(c.color(Segment::User), default_color(Segment::User));
        assert_eq!(c.color(Segment::Clock), default_color(Segment::Clock));
    }

    #[test]
    fn status_fail_and_git_dirty_colors() {
        let c = cfg(&[]);
        assert_eq!(c.status_fail(), Color::new(52, 196));
        assert_eq!(c.git_dirty(), Color::new(236, 229));
        let c = cfg(&[("CAKE_PROMPT_COLOR_FAIL", "196,231")]);
        assert_eq!(c.status_fail(), Color::new(196, 231));
        let c = cfg(&[("CAKE_PROMPT_COLOR_FAIL", "not-a-color")]);
        assert_eq!(c.status_fail(), Color::new(52, 196));
    }
}
