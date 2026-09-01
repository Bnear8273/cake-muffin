//! Parse `git status --porcelain=v2 -b` output into a structured [`GitStatus`].

use alloc::borrow::ToOwned;
use alloc::string::String;

/// Working-tree state as reported by `git status --porcelain=v2 -b`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitStatus {
    /// Branch name, or `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Files modified in the index (staged).
    pub staged: usize,
    /// Files modified in the worktree (unstaged).
    pub unstaged: usize,
    /// Untracked files/directories.
    pub untracked: usize,
    /// Commits ahead of the upstream.
    pub ahead: usize,
    /// Commits behind the upstream.
    pub behind: usize,
}

impl GitStatus {
    /// Whether anything is staged, modified or untracked.
    pub fn dirty(&self) -> bool {
        self.staged + self.unstaged + self.untracked > 0
    }
}

/// Parse porcelain-v2 output. Returns `None` when the input does not look
/// like a git status dump for a repository (no `# branch.head` header line).
pub fn parse_status(text: &str) -> Option<GitStatus> {
    let mut st = GitStatus::default();
    let mut in_repo = false;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            in_repo = true;
            st.branch = if rest == "(detached)" {
                None
            } else {
                Some(rest.trim().to_owned())
            };
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let mut parts = rest.split_whitespace();
            st.ahead = parse_ab(parts.next());
            st.behind = parse_ab(parts.next());
        } else if line.starts_with('?') {
            st.untracked += 1;
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // `1 XY ...` / `2 XY ...`: X = index status, Y = worktree status.
            let b = line.as_bytes();
            if b.len() >= 4 {
                if b[2] != b'.' {
                    st.staged += 1;
                }
                if b[3] != b'.' {
                    st.unstaged += 1;
                }
            }
        }
    }

    if !in_repo {
        return None;
    }
    Some(st)
}

/// Parse an ahead/behind token like `+2` or `-3` (ignoring the sign).
fn parse_ab(s: Option<&str>) -> usize {
    s.and_then(|s| s.strip_prefix('+').or_else(|| s.strip_prefix('-')))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    const CLEAN: &str = "\
# branch.oid 9a8b7c6d5e4f3a2b1c0d
# branch.head main
# branch.upstream origin/main
# branch.ab +0 -0
";

    #[test]
    fn clean_repo() {
        let st = parse_status(CLEAN).unwrap();
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(
            (st.staged, st.unstaged, st.untracked, st.ahead, st.behind),
            (0, 0, 0, 0, 0)
        );
        assert!(!st.dirty());
    }

    #[test]
    fn unstaged_vs_staged_vs_both() {
        // .M = worktree modified only; M. = staged only; MM = both.
        let text = format!(
            "{CLEAN}1 .M N... 100644 100644 100644 abc file1\n\
             1 M. N... 100644 100644 100644 abc file2\n\
             1 MM N... 100644 100644 100644 abc file3\n"
        );
        let st = parse_status(&text).unwrap();
        assert_eq!(st.unstaged, 2); // file1 (.M) + file3 (MM)
        assert_eq!(st.staged, 2); // file2 (M.) + file3 (MM)
        assert!(st.dirty());
    }

    #[test]
    fn untracked_counted() {
        let text = format!("{CLEAN}? newfile.txt\n? dir/\n");
        let st = parse_status(&text).unwrap();
        assert_eq!(st.untracked, 2);
        assert!(st.dirty());
    }

    #[test]
    fn ahead_behind() {
        let text = CLEAN.replace("# branch.ab +0 -0", "# branch.ab +3 -1");
        let st = parse_status(&text).unwrap();
        assert_eq!(st.ahead, 3);
        assert_eq!(st.behind, 1);
    }

    #[test]
    fn no_upstream_has_zero_ab() {
        let text = "\
# branch.oid 9a8b7c6d5e4f3a2b1c0d
# branch.head dev
";
        let st = parse_status(text).unwrap();
        assert_eq!((st.ahead, st.behind), (0, 0));
        assert_eq!(st.branch.as_deref(), Some("dev"));
    }

    #[test]
    fn detached_head() {
        let text = "\
# branch.oid 9a8b7c6d5e4f3a2b1c0d
# branch.head (detached)
1 .M N... 100644 100644 100644 abc f
";
        let st = parse_status(text).unwrap();
        assert_eq!(st.branch, None);
        assert_eq!(st.unstaged, 1);
    }

    #[test]
    fn renames_count_via_xy() {
        // 2 R. = staged rename.
        let text = format!("{CLEAN}2 R. N... 100644 100644 100644 abc old.txt\tnew.txt\n");
        let st = parse_status(&text).unwrap();
        assert_eq!(st.staged, 1);
        assert_eq!(st.unstaged, 0);
    }

    #[test]
    fn not_a_repo() {
        assert_eq!(parse_status(""), None);
        assert_eq!(parse_status("fatal: not a git repository\n"), None);
        assert_eq!(parse_status("# some other header\n"), None);
    }

    #[test]
    fn initial_commit_still_repo() {
        let text = "\
# branch.oid (initial)
# branch.head main
";
        assert_eq!(parse_status(text).unwrap().branch.as_deref(), Some("main"));
    }
}
