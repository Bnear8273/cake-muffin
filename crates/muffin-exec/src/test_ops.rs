//! Shared test-operator evaluation for `[ ... ]` (builtins) and `[[ ... ]]`
//! (cond). Eliminates the duplicated file-test and comparison logic that
//! previously existed in both files.

use muffin_platform::FileInfo;

/// Parse a string as `i64`, trimming whitespace. Returns `0` on failure.
pub(crate) fn parse_num(s: &str) -> i64 {
    s.trim().parse().unwrap_or(0)
}

/// Evaluate a unary file-test operator against pre-fetched [`FileInfo`].
///
/// Operators that don't depend on `FileInfo` (`-n`, `-z`, `-t`) are not
/// handled here — callers handle those directly.
pub(crate) fn eval_unary_file_test(info: &FileInfo, op: &str, my_uid: u32, my_gid: u32) -> bool {
    match op {
        "-e" => info.exists,
        "-f" => info.is_file,
        "-d" => info.is_dir,
        "-r" => info.is_readable,
        "-w" => info.is_writable,
        "-x" => info.is_executable,
        "-s" => info.size > 0,
        "-L" | "-h" => info.is_symlink,
        "-S" => info.is_socket,
        "-b" => info.is_block_device,
        "-c" => info.is_char_device,
        "-p" => info.is_fifo,
        "-u" => info.has_suid,
        "-g" => info.has_sgid,
        "-k" => info.has_sticky,
        "-N" => info.mtime > info.atime,
        "-O" => info.uid == my_uid,
        "-G" => info.gid == my_gid,
        _ => false,
    }
}

/// Evaluate a binary file-test operator against two pre-fetched [`FileInfo`]
/// values.
pub(crate) fn eval_binary_file_test(
    info_lhs: &FileInfo,
    info_rhs: &FileInfo,
    op: &str,
) -> Option<bool> {
    match op {
        "-nt" => Some(info_lhs.mtime > info_rhs.mtime),
        "-ot" => Some(info_lhs.mtime < info_rhs.mtime),
        "-ef" => Some(info_lhs.dev == info_rhs.dev && info_lhs.ino == info_rhs.ino),
        _ => None,
    }
}
