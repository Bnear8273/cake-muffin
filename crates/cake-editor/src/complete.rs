//! Completion logic: longest-common-prefix computation and state tracking
//! for the two-tap Tab flow (first tap → LCP, second tap → list).

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp;

/// State for the two-tap Tab flow.
#[derive(Debug, Clone)]
pub struct CompletionState {
    /// Byte offset of the word being completed.
    pub start: usize,
    /// Full list of candidates (from `Services::complete`).
    pub candidates: Vec<crate::Candidate>,
    /// Whether the candidate list has been shown to the user (second tap).
    pub list_shown: bool,
}

/// Compute the longest common prefix of a sequence of strings.
///
/// Returns an empty string when the iterator is empty or has no common prefix.
pub fn longest_common_prefix<'a>(mut iter: impl Iterator<Item = &'a str>) -> String {
    let Some(first) = iter.next() else {
        return String::new();
    };
    let mut prefix = first.len();
    for s in iter {
        let common = first
            .as_bytes()
            .iter()
            .zip(s.as_bytes())
            .take_while(|(a, b)| a == b)
            .count();
        prefix = cmp::min(prefix, common);
        if prefix == 0 {
            break;
        }
    }
    first[..prefix].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Candidate;

    #[test]
    fn lcp_empty() {
        assert_eq!(longest_common_prefix(core::iter::empty::<&str>()), "");
    }

    #[test]
    fn lcp_single() {
        let c = [Candidate {
            display: "".into(),
            replacement: "ls".into(),
        }];
        assert_eq!(
            longest_common_prefix(c.iter().map(|c| c.replacement.as_str())),
            "ls"
        );
    }

    #[test]
    fn lcp_multiple() {
        let c = [
            Candidate {
                display: "".into(),
                replacement: "ls".into(),
            },
            Candidate {
                display: "".into(),
                replacement: "less".into(),
            },
            Candidate {
                display: "".into(),
                replacement: "ln".into(),
            },
        ];
        assert_eq!(
            longest_common_prefix(c.iter().map(|c| c.replacement.as_str())),
            "l"
        );
    }

    #[test]
    fn lcp_exact_match() {
        let c = [
            Candidate {
                display: "".into(),
                replacement: "export".into(),
            },
            Candidate {
                display: "".into(),
                replacement: "export".into(),
            },
        ];
        assert_eq!(
            longest_common_prefix(c.iter().map(|c| c.replacement.as_str())),
            "export"
        );
    }

    #[test]
    fn lcp_no_common() {
        let c = [
            Candidate {
                display: "".into(),
                replacement: "abc".into(),
            },
            Candidate {
                display: "".into(),
                replacement: "xyz".into(),
            },
        ];
        assert_eq!(
            longest_common_prefix(c.iter().map(|c| c.replacement.as_str())),
            ""
        );
    }
}
