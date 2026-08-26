use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// Flags controlling the behaviour of a shell variable.
///
/// Modeled after fish's `EnvVarFlags` and bash's variable attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EnvVarFlags(u8);

impl EnvVarFlags {
    pub const NONE: EnvVarFlags = EnvVarFlags(0);
    /// Variable is exported to child processes (bash `export`).
    pub const EXPORT: EnvVarFlags = EnvVarFlags(1 << 0);
    /// Variable cannot be modified (`readonly`).
    pub const READONLY: EnvVarFlags = EnvVarFlags(1 << 1);
    /// Path-like list variable: values joined with `:` when exported (PATH,
    /// CDPATH, MANPATH, ...).
    pub const PATHVAR: EnvVarFlags = EnvVarFlags(1 << 2);

    pub fn contains(self, other: EnvVarFlags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: EnvVarFlags) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: EnvVarFlags) {
        self.0 &= !other.0;
    }
}

impl core::ops::BitOr for EnvVarFlags {
    type Output = EnvVarFlags;

    fn bitor(self, rhs: EnvVarFlags) -> EnvVarFlags {
        EnvVarFlags(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for EnvVarFlags {
    fn bitor_assign(&mut self, rhs: EnvVarFlags) {
        self.0 |= rhs.0;
    }
}

pub const PATH_DELIMITER: char = ':';

/// The value of a shell variable.
///
/// Immutable: values are shared cheaply via `Arc`, and "setting" produces a
/// new `EnvVar`. This mirrors fish's copy-on-write `EnvVar`.
#[derive(Debug, Clone)]
pub struct EnvVar {
    values: Arc<[String]>,
    flags: EnvVarFlags,
}

impl EnvVar {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            values: Arc::from([value.into()]),
            flags: EnvVarFlags::NONE,
        }
    }

    pub fn new_list(values: Vec<String>) -> Self {
        Self {
            values: Arc::from(values),
            flags: EnvVarFlags::NONE,
        }
    }

    /// A path-list variable (e.g. `PATH`).
    pub fn from_path(paths: Vec<String>) -> Self {
        Self {
            values: Arc::from(paths),
            flags: EnvVarFlags::PATHVAR,
        }
    }

    /// All values of the variable.
    pub fn values(&self) -> &[String] {
        &self.values
    }

    /// The first value, or `""` if the variable is empty.
    pub fn value(&self) -> &str {
        self.values.first().map(String::as_str).unwrap_or("")
    }

    pub fn flags(&self) -> EnvVarFlags {
        self.flags
    }

    pub fn is_exported(&self) -> bool {
        self.flags.contains(EnvVarFlags::EXPORT)
    }

    pub fn is_readonly(&self) -> bool {
        self.flags.contains(EnvVarFlags::READONLY)
    }

    pub fn is_pathvar(&self) -> bool {
        self.flags.contains(EnvVarFlags::PATHVAR)
    }

    /// The delimiter used when joining the values of a list variable.
    pub fn delimiter(&self) -> char {
        if self.is_pathvar() {
            PATH_DELIMITER
        } else {
            // Non-path lists are joined with the unit separator so values
            // containing spaces survive round-tripping through the env.
            '\u{1f}'
        }
    }

    /// A copy with the given flags.
    pub fn set_flags(&self, flags: EnvVarFlags) -> Self {
        Self {
            values: self.values.clone(),
            flags,
        }
    }

    /// The string form exported to a child process's environment.
    pub fn as_env_string(&self) -> String {
        if self.is_pathvar() {
            self.values.join(&PATH_DELIMITER.to_string())
        } else {
            self.values.join(&self.delimiter().to_string())
        }
    }
}

impl From<&str> for EnvVar {
    fn from(s: &str) -> Self {
        EnvVar::new(s)
    }
}

impl From<String> for EnvVar {
    fn from(s: String) -> Self {
        EnvVar::new(s)
    }
}
