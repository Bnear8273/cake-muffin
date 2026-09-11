//! Pathname pattern matching (`*`, `?`, `[...]`, and with `extglob` the
//! `?(...)` `*(...)` `+(...)` `@(...)` `!(...)` groups) used by `case`
//! patterns and, in M2c, by glob expansion.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Match `text` against shell pattern `pat`.
///
/// Supports `*` (any sequence), `?` (one char) and `[...]` character classes
/// with `!`/`^` negation and `a-z` ranges, plus extglob groups when
/// `extglob` is set.
pub fn glob_match(pat: &str, text: &str) -> bool {
    match_inner(pat.as_bytes(), text.as_bytes(), false, false)
}

/// Case-insensitive glob match (ASCII folding), for `shopt nocaseglob`.
pub fn glob_match_case(pat: &str, text: &str) -> bool {
    match_inner(pat.as_bytes(), text.as_bytes(), true, false)
}

/// Match with extglob groups toggled by `extglob` (`shopt -s extglob`).
pub fn glob_match_ext(pat: &str, text: &str, nocase: bool, extglob: bool) -> bool {
    match_inner(pat.as_bytes(), text.as_bytes(), nocase, extglob)
}

fn fold(b: u8) -> u8 {
    b.to_ascii_lowercase()
}

// --- pattern AST ---------------------------------------------------------

#[derive(Debug, Clone)]
enum PNode {
    /// A literal byte.
    Lit(u8),
    /// `?` — any single byte.
    Any,
    /// `*` — any run of bytes.
    Star,
    /// `[...]`
    Class {
        negated: bool,
        ranges: Vec<(u8, u8)>,
    },
    /// `?(p)` `*(p)` `+(p)` `@(p)` `!(p)`.
    Group { op: u8, alts: Vec<Vec<PNode>> },
}

/// Parse a pattern into nodes. Stops at an unescaped `|` or `)` when
/// `top` is false (a group alternative).
fn parse_nodes(pat: &[u8], i: &mut usize, extglob: bool, top: bool) -> Vec<PNode> {
    let mut nodes = Vec::new();
    while *i < pat.len() {
        let c = pat[*i];
        match c {
            b'\\' if *i + 1 < pat.len() => {
                nodes.push(PNode::Lit(pat[*i + 1]));
                *i += 2;
            }
            b'?' => {
                nodes.push(PNode::Any);
                *i += 1;
            }
            b'*' => {
                nodes.push(PNode::Star);
                *i += 1;
            }
            b'[' => nodes.push(parse_class(pat, i)),
            b')' | b'|' if !top => break,
            b'(' if extglob
                && *i > 0
                && matches!(pat[*i - 1], b'?' | b'*' | b'+' | b'@' | b'!') =>
            {
                let op = pat[*i - 1];
                nodes.pop(); // drop the opener byte parsed as a literal
                *i += 1;
                let mut alts = Vec::new();
                loop {
                    let alt = parse_nodes(pat, i, extglob, false);
                    alts.push(alt);
                    if *i >= pat.len() || pat[*i] != b'|' {
                        break;
                    }
                    *i += 1;
                }
                // Consume the closing `)` if present; tolerate unterminated.
                if *i < pat.len() && pat[*i] == b')' {
                    *i += 1;
                }
                nodes.push(PNode::Group { op, alts });
            }
            _ => {
                nodes.push(PNode::Lit(c));
                *i += 1;
            }
        }
    }
    nodes
}

fn parse_class(pat: &[u8], i: &mut usize) -> PNode {
    let start = *i;
    *i += 1; // '['
    let negated = *i < pat.len() && (pat[*i] == b'!' || pat[*i] == b'^');
    if negated {
        *i += 1;
    }
    let mut ranges: Vec<(u8, u8)> = Vec::new();
    let mut first = true;
    while *i < pat.len() {
        if pat[*i] == b']' && !first {
            *i += 1;
            return PNode::Class { negated, ranges };
        }
        first = false;
        // POSIX character class: [[:alpha:]], [:digit:], etc.
        if pat[*i] == b'[' && *i + 1 < pat.len() && pat[*i + 1] == b':' {
            // Look ahead for [:name:]]
            let class_start = *i + 2;
            if let Some(class_end) = find_posix_class(pat, class_start) {
                let class_name = &pat[class_start..class_end];
                // Add the POSIX class ranges.
                if let Some(class_ranges) = posix_class_ranges(class_name) {
                    ranges.extend(class_ranges);
                }
                // Skip past ":]" to the closing `]` of the outer class.
                *i = class_end + 2; // skip past ']'
                if *i < pat.len() && pat[*i] == b']' {
                    *i += 1;
                    return PNode::Class { negated, ranges };
                }
                continue;
            }
        }
        if pat[*i] == b'\\' && *i + 1 < pat.len() {
            *i += 1;
        }
        if *i + 2 < pat.len() && pat[*i + 1] == b'-' && pat[*i + 2] != b']' {
            ranges.push((pat[*i], pat[*i + 2]));
            *i += 3;
        } else {
            ranges.push((pat[*i], pat[*i]));
            *i += 1;
        }
    }
    // Unterminated class: treat `[` as a literal.
    *i = start + 1;
    PNode::Class {
        negated: false,
        ranges: vec![(b'[', b'[')],
    }
}

/// Find the closing `:` in a POSIX class `[:name:]` inside a bracket expression.
/// Returns the index of the closing `]` (not the `:` before it).
fn find_posix_class(pat: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < pat.len() {
        if pat[i] == b':' && i + 1 < pat.len() && pat[i + 1] == b']' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Map a POSIX character class name to its byte ranges.
fn posix_class_ranges(name: &[u8]) -> Option<Vec<(u8, u8)>> {
    match name {
        b"alnum" => Some(vec![(b'0', b'9'), (b'A', b'Z'), (b'a', b'z')]),
        b"alpha" => Some(vec![(b'A', b'Z'), (b'a', b'z')]),
        b"blank" => Some(vec![(b' ', b' '), (b'\t', b'\t')]),
        b"cntrl" => Some(vec![(b'\x00', b'\x1f'), (b'\x7f', b'\x7f')]),
        b"digit" => Some(vec![(b'0', b'9')]),
        b"graph" => Some(vec![(b'!', b'~')]),
        b"lower" => Some(vec![(b'a', b'z')]),
        b"print" => Some(vec![(b' ', b'~')]),
        b"punct" => Some(vec![(b'!', b'/'), (b':', b'@'), (b'[', b'`'), (b'{', b'~')]),
        b"space" => Some(vec![
            (b' ', b' '),
            (b'\t', b'\t'),
            (b'\n', b'\n'),
            (b'\x0b', b'\x0b'),
            (b'\x0c', b'\x0c'),
            (b'\r', b'\r'),
        ]),
        b"upper" => Some(vec![(b'A', b'Z')]),
        b"xdigit" => Some(vec![(b'0', b'9'), (b'A', b'F'), (b'a', b'f')]),
        _ => None,
    }
}

fn class_matches(c: u8, negated: bool, ranges: &[(u8, u8)], nocase: bool) -> bool {
    let c = if nocase { fold(c) } else { c };
    let mut matched = false;
    for (lo, hi) in ranges {
        let (lo, hi) = if nocase {
            (fold(*lo), fold(*hi))
        } else {
            (*lo, *hi)
        };
        if lo <= c && c <= hi {
            matched = true;
            break;
        }
    }
    matched != negated
}

// --- matching ------------------------------------------------------------

/// Does `nodes[i..]` match `text[t..end]`? Failed states are memoised.
fn match_nodes(
    nodes: &[PNode],
    text: &[u8],
    t: usize,
    end: usize,
    i: usize,
    memo: &mut BTreeSet<(usize, usize)>,
    nocase: bool,
) -> bool {
    if i == nodes.len() {
        return t == end;
    }
    if t > end {
        return false;
    }
    if memo.contains(&(i, t)) {
        return false;
    }
    let ok = match &nodes[i] {
        PNode::Lit(c) => {
            t < end
                && (*c == text[t] || (nocase && fold(*c) == fold(text[t])))
                && match_nodes(nodes, text, t + 1, end, i + 1, memo, nocase)
        }
        PNode::Any => t < end && match_nodes(nodes, text, t + 1, end, i + 1, memo, nocase),
        PNode::Star => {
            let mut k = end;
            loop {
                if match_nodes(nodes, text, t + k, end, i + 1, memo, nocase) {
                    break true;
                }
                if k == 0 {
                    break false;
                }
                k -= 1;
            }
        }
        PNode::Class { negated, ranges } => {
            t < end
                && class_matches(text[t], *negated, ranges, nocase)
                && match_nodes(nodes, text, t + 1, end, i + 1, memo, nocase)
        }
        PNode::Group { op, alts } => match op {
            // `@(p)`: exactly one alternative.
            b'@' => (t..=end).any(|k| {
                (k > t)
                    && alt_exact(alts, text, t, k, nocase)
                    && match_nodes(nodes, text, k, end, i + 1, memo, nocase)
            }),
            // `?(p)`: zero or one.
            b'?' => {
                match_nodes(nodes, text, t, end, i + 1, memo, nocase)
                    || (t..=end).any(|k| {
                        (k > t)
                            && alt_exact(alts, text, t, k, nocase)
                            && match_nodes(nodes, text, k, end, i + 1, memo, nocase)
                    })
            }
            // `*(p)`: zero or more.
            b'*' => {
                match_nodes(nodes, text, t, end, i + 1, memo, nocase)
                    || (t + 1..=end).any(|k| {
                        alt_exact(alts, text, t, k, nocase)
                            && match_nodes(nodes, text, k, end, i, memo, nocase)
                    })
            }
            // `+(p)`: one or more.
            b'+' => (t + 1..=end).any(|k| {
                alt_exact(alts, text, t, k, nocase)
                    && (match_nodes(nodes, text, k, end, i + 1, memo, nocase)
                        || match_nodes(nodes, text, k, end, i, memo, nocase))
            }),
            // `!(p)`: any string not matching an alternative.
            b'!' => (t..=end).any(|k| {
                !alt_exact(alts, text, t, k, nocase)
                    && match_nodes(nodes, text, k, end, i + 1, memo, nocase)
            }),
            _ => false,
        },
    };
    if !ok {
        memo.insert((i, t));
    }
    ok
}

/// Does any alternative exactly match `text[t..k]`?
fn alt_exact(alts: &[Vec<PNode>], text: &[u8], t: usize, k: usize, nocase: bool) -> bool {
    alts.iter().any(|alt| {
        let mut memo = BTreeSet::new();
        match_nodes(alt, text, t, k, 0, &mut memo, nocase)
    })
}

fn match_inner(pat: &[u8], text: &[u8], nocase: bool, extglob: bool) -> bool {
    let nodes = parse_nodes(pat, &mut 0, extglob, true);
    let mut memo = BTreeSet::new();
    match_nodes(&nodes, text, 0, text.len(), 0, &mut memo, nocase)
}

/// Does `text` contain any glob metacharacter?
pub fn has_glob_chars(s: &str) -> bool {
    has_glob_chars_ext(s, false)
}

/// `has_glob_chars` with extglob openers taken into account.
pub fn has_glob_chars_ext(s: &str, extglob: bool) -> bool {
    let b = s.as_bytes();
    for i in 0..b.len() {
        match b[i] {
            b'*' | b'?' | b'[' => return true,
            b'(' if extglob && i > 0 && matches!(b[i - 1], b'?' | b'*' | b'+' | b'@' | b'!') => {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Expand a glob pattern against the filesystem.
///
/// Walks each path component in turn, matching entries with [`glob_match`].
/// `*`/`?`/`[...]` do not match a leading `.` unless the pattern component
/// starts with one (or `dotglob` is set). `**` matches any number of
/// directories (including none) recursively. Results are sorted (bash sorts
/// lexicographically). Returns an empty `Vec` when nothing matches (the
/// caller keeps the literal pattern).
pub fn expand_glob(
    platform: &dyn cake_platform::ProcessModel,
    pattern: &str,
    dotglob: bool,
    nocaseglob: bool,
    extglob: bool,
    globstar: bool,
) -> Vec<String> {
    let p = platform;
    let sep = p.path_separator();
    let abs = pattern
        .chars()
        .next()
        .is_some_and(|c| p.is_path_separator(c));
    let parts: Vec<&str> = pattern
        .trim_start_matches(|c: char| p.is_path_separator(c))
        .split(|c: char| p.is_path_separator(c))
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Vec::new();
    }
    // Root prefix for absolute patterns (`/` on Unix, `\` on Windows).
    let root = alloc::format!("{sep}");
    // Accumulated matched path prefixes (`""` = relative to cwd).
    let mut results: Vec<String> = vec![String::new()];
    // Tracks whether each result came from a globstar `**` expansion
    // (used to allow the Err fallback for files in subsequent components).
    let mut from_globstar: Vec<bool> = vec![false];

    for (i, part) in parts.iter().enumerate() {
        let mut next: Vec<String> = Vec::new();
        let mut next_gs: Vec<bool> = Vec::new();

        // `**` — recursive directory match (only when `globstar` is set):
        // collect all entries under each base, then skip to the next
        // component (the remaining pattern is matched against every entry
        // found recursively). When `globstar` is off, `**` behaves like `*`.
        if *part == "**" && globstar {
            for base in &results {
                let dir = if i == 0 && !abs {
                    "."
                } else if base.is_empty() {
                    root.as_str()
                } else {
                    base
                };
                let mut entries = Vec::new();
                // If ** is not the last component, only collect directories
                // so the next component can iterate their contents.
                // Also include the base dir itself for zero-length ** matches.
                let dirs_only = i + 1 < parts.len();
                collect_recursive(p, dir, &mut entries, sep, dirs_only, dotglob);
                // For zero-length ** matches: include the base dir itself.
                if !dir.is_empty()
                    && dir != "."
                    && !entries.contains(&alloc::string::ToString::to_string(dir))
                {
                    entries.insert(0, alloc::string::ToString::to_string(dir));
                }
                // collect_recursive already builds full paths from `dir`.
                // For relative patterns rooted at ".", strip the "./" prefix.
                if !abs && (i == 0 && base.is_empty()) {
                    let dot_sep = alloc::format!(".{sep}");
                    for e in entries {
                        if let Some(stripped) = e.strip_prefix(dot_sep.as_str()) {
                            next.push(alloc::string::ToString::to_string(stripped));
                            next_gs.push(true);
                        } else {
                            next.push(e);
                            next_gs.push(true);
                        }
                    }
                } else {
                    let n = entries.len();
                    next.extend(entries);
                    next_gs.resize(next_gs.len() + n, true);
                }
            }
            results = next;
            from_globstar = next_gs;
            continue;
        }

        for (idx, base) in results.iter().enumerate() {
            let dir = if i == 0 && !abs {
                "."
            } else if base.is_empty() {
                root.as_str()
            } else {
                base
            };
            let entries = match p.read_dir(dir) {
                Ok(e) => e,
                Err(_) => {
                    // If base is a file from a globstar ** expansion, try matching
                    // its filename against the current component.
                    if from_globstar[idx]
                        && i > 0
                        && p.file_info(base).exists
                        && !p.file_info(base).is_dir
                        && base.rsplit(sep).next().is_some_and(|fname| {
                            has_glob_chars_ext(part, extglob)
                                && glob_match_ext(part, fname, nocaseglob, extglob)
                        })
                    {
                        next.push(base.clone());
                        next_gs.push(false);
                    }
                    continue;
                }
            };
            let join = |name: &str| {
                let mut s = String::new();
                if i > 0 || abs {
                    s.push_str(base);
                    if !s.is_empty() || abs {
                        s.push(sep);
                    }
                }
                s.push_str(name);
                s
            };
            if has_glob_chars_ext(part, extglob) {
                for name in &entries {
                    if !dotglob && name.starts_with('.') && !part.starts_with('.') {
                        continue;
                    }
                    if glob_match_ext(part, name, nocaseglob, extglob) {
                        next.push(join(name));
                        next_gs.push(false);
                    }
                }
            } else if *part == "." {
                next.push(base.clone());
                next_gs.push(false);
            } else if *part == ".." {
                let mut s = String::new();
                s.push_str(base);
                if !s.is_empty() || abs {
                    s.push(sep);
                }
                s.push_str("..");
                next.push(s);
                next_gs.push(false);
            } else if entries.iter().any(|e| e == part) {
                next.push(join(part));
                next_gs.push(false);
            }
        }
        results = next;
        from_globstar = next_gs;
        if results.is_empty() {
            break;
        }
    }
    results.sort();
    results
}

/// Recursively collect all entries under `dir`. When `dirs_only` is true,
/// only directories are collected (but NOT `dir` itself).
/// When `dotglob` is false, leading-dot names are excluded.
fn collect_recursive(
    p: &dyn cake_platform::Platform,
    dir: &str,
    out: &mut Vec<String>,
    sep: char,
    dirs_only: bool,
    dotglob: bool,
) {
    let entries = match p.read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for name in &entries {
        if !dotglob && name.starts_with('.') {
            continue;
        }
        let full = if dir.is_empty() || dir == "." {
            alloc::string::ToString::to_string(name)
        } else {
            alloc::format!("{dir}{sep}{name}")
        };
        let is_dir = p.file_info(&full).is_dir;
        if !dirs_only || is_dir {
            out.push(full.clone());
        }
        if is_dir {
            collect_recursive(p, &full, out, sep, dirs_only, dotglob);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extglob_off_is_literal() {
        assert!(!glob_match("?(foo|bar).txt", "bar.txt"));
        assert!(!glob_match("?(foo|bar).txt", "foo.txt"));
        assert!(glob_match("?(foo|bar).txt", "?(foo|bar).txt"));
    }

    #[test]
    fn extglob_groups() {
        let m = |pat: &str, s: &str| glob_match_ext(pat, s, false, true);
        assert!(m("@(foo|bar)", "foo"));
        assert!(m("@(foo|bar)", "bar"));
        assert!(!m("@(foo|bar)", "baz"));
        assert!(m("?(foo|bar)", ""));
        assert!(m("?(foo|bar)", "foo"));
        assert!(m("*(foo)", ""));
        assert!(m("*(foo)", "foofoofoo"));
        assert!(!m("*(foo)", "foob"));
        assert!(m("+(foo)", "foo"));
        assert!(!m("+(foo)", ""));
        assert!(m("!(foo)", "bar"));
        assert!(!m("!(foo)", "foo"));
        assert!(m("!(foo)", ""));
        assert!(m("+(foo|bar).txt", "barfoo.txt"));
        assert!(m("!(b)*", "foo.txt"));
        // `!(b)` can match any prefix that is not exactly `b` (bash).
        assert!(m("!(b)*", "bar.txt"));
        assert!(!m("!(b)", "b"));
    }

    #[test]
    fn posix_char_classes() {
        assert!(glob_match("[[:alpha:]]*.txt", "a.txt"));
        assert!(glob_match("[[:alpha:]]*.txt", "ABC.txt"));
        assert!(!glob_match("[[:alpha:]]*.txt", "1.txt"));
        assert!(glob_match("[[:digit:]]", "7"));
        assert!(!glob_match("[[:digit:]]", "a"));
        assert!(glob_match("[[:upper:]]", "Z"));
        assert!(!glob_match("[[:upper:]]", "z"));
        assert!(glob_match("?[[:alnum:]]", "a1"));
    }

    fn mock_dir(entries: &[&str]) -> &'static cake_platform_mock::MockPlatform {
        use alloc::boxed::Box;
        let p: &'static cake_platform_mock::MockPlatform =
            Box::leak(Box::new(cake_platform_mock::MockPlatform::new()));
        p.set_dir_entries(".", entries);
        p
    }

    fn names(v: &[&str]) -> Vec<String> {
        use alloc::string::ToString;
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn expand_glob_matches_mock_dir() {
        let p = mock_dir(&["a.txt", "b.txt", "c.rs", ".hidden"]);
        // `*` matches every non-dot entry.
        assert_eq!(
            expand_glob(p, "*.txt", false, false, false, false),
            names(&["a.txt", "b.txt"])
        );
        // `?` matches exactly one char.
        assert_eq!(
            expand_glob(p, "?.txt", false, false, false, false),
            names(&["a.txt", "b.txt"])
        );
        // `[...]` character class.
        assert_eq!(
            expand_glob(p, "[ab].txt", false, false, false, false),
            names(&["a.txt", "b.txt"])
        );
        assert_eq!(
            expand_glob(p, "[a].txt", false, false, false, false),
            names(&["a.txt"])
        );
    }

    #[test]
    fn expand_glob_dotfiles_need_dot_or_dotglob() {
        let p = mock_dir(&["a.txt", ".hidden"]);
        // Leading-dot entries are skipped unless the pattern starts with one.
        assert_eq!(
            expand_glob(p, "*", false, false, false, false),
            names(&["a.txt"])
        );
        assert_eq!(
            expand_glob(p, ".*", false, false, false, false),
            names(&[".hidden"])
        );
        // ... or dotglob is set.
        let mut both = expand_glob(p, "*", true, false, false, false);
        both.sort();
        assert_eq!(both, names(&[".hidden", "a.txt"]));
    }

    #[test]
    fn expand_glob_no_match_is_empty() {
        let p = mock_dir(&["a.txt"]);
        assert!(expand_glob(p, "*.md", false, false, false, false).is_empty());
        // Unconfigured directories read as errors, so nothing matches.
        let q = mock_dir(&[]);
        assert!(expand_glob(q, "/etc/*.conf", false, false, false, false).is_empty());
    }

    #[test]
    fn expand_glob_absolute_pattern() {
        use alloc::boxed::Box;
        let p: &'static cake_platform_mock::MockPlatform =
            Box::leak(Box::new(cake_platform_mock::MockPlatform::new()));
        p.set_dir_entries("/", &["etc", "bin"]);
        p.set_dir_entries("/etc", &["hosts", "fstab"]);
        assert_eq!(
            expand_glob(p, "/etc/h*", false, false, false, false),
            names(&["/etc/hosts"])
        );
    }
}
