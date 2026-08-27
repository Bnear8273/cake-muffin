use alloc::borrow::ToOwned;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use crate::env_var::EnvVar;

/// A single lexical scope: a set of variable bindings.
#[derive(Debug, Clone, Default)]
struct Scope {
    vars: BTreeMap<String, EnvVar>,
}

/// Error returned when setting a variable fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvSetError {
    /// Variable is marked readonly.
    Readonly(String),
    /// The variable name is invalid.
    Invalid(String),
}

impl core::fmt::Display for EnvSetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EnvSetError::Readonly(name) => write!(f, "{name}: readonly variable"),
            EnvSetError::Invalid(name) => write!(f, "{name}: not a valid identifier"),
        }
    }
}

/// A stack of variable scopes.
///
/// Modeled after fish's `EnvStack`. The bottom scope holds globals (seeded
/// from the process environment by the shell driver and variables exported
/// with `export`); local scopes are pushed for function calls and command
/// blocks (`( )`, `{ }`). Lookup walks from the innermost scope outward.
#[derive(Debug, Clone)]
pub struct EnvStack {
    globals: Scope,
    locals: Vec<Scope>,
}

impl EnvStack {
    pub fn new() -> Self {
        Self {
            globals: Scope::default(),
            locals: Vec::new(),
        }
    }

    /// Seed the global scope from process-environment pairs.
    ///
    /// The caller (the std shell driver) is responsible for splitting
    /// path-list variables such as `PATH` into their components.
    pub fn seed_globals(&mut self, vars: impl IntoIterator<Item = (String, EnvVar)>) {
        for (name, var) in vars {
            self.globals.vars.insert(name, var);
        }
    }

    /// Enter a new local scope.
    pub fn push_scope(&mut self) {
        self.locals.push(Scope::default());
    }

    /// Leave the innermost local scope.
    pub fn pop_scope(&mut self) {
        self.locals.pop();
    }

    /// The number of active local scopes.
    pub fn depth(&self) -> usize {
        self.locals.len()
    }

    /// Look up a variable, searching from the innermost scope outward.
    pub fn get(&self, name: &str) -> Option<&EnvVar> {
        for scope in self.locals.iter().rev() {
            if let Some(var) = scope.vars.get(name) {
                return Some(var);
            }
        }
        self.globals.vars.get(name)
    }

    /// Look up a variable in the global scope only.
    pub fn get_global(&self, name: &str) -> Option<&EnvVar> {
        self.globals.vars.get(name)
    }

    /// All visible variable names, innermost-first.
    pub fn get_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for scope in self.locals.iter().rev().chain(core::iter::once(&self.globals)) {
            for name in scope.vars.keys() {
                if seen.insert(name.as_str()) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Exported variables as (name, value) pairs, for `export` with no args.
    pub fn get_names_exported(&self) -> Vec<(&str, &EnvVar)> {
        let mut out: Vec<(&str, &EnvVar)> = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for scope in self.locals.iter().rev().chain(core::iter::once(&self.globals)) {
            for (name, var) in &scope.vars {
                if var.is_exported() && seen.insert(name.as_str()) {
                    out.push((name.as_str(), var));
                }
            }
        }
        out
    }

    /// Set a variable in the innermost scope, respecting readonly bindings.
    pub fn set(&mut self, name: &str, var: EnvVar) -> Result<(), EnvSetError> {
        if let Some(existing) = self.get(name)
            && existing.is_readonly()
        {
            return Err(EnvSetError::Readonly(name.to_owned()));
        }
        self.top_scope_mut().vars.insert(name.to_owned(), var);
        Ok(())
    }

    /// Set a variable in the global scope.
    pub fn set_global(&mut self, name: &str, var: EnvVar) -> Result<(), EnvSetError> {
        if let Some(existing) = self.globals.vars.get(name)
            && existing.is_readonly()
        {
            return Err(EnvSetError::Readonly(name.to_owned()));
        }
        self.globals.vars.insert(name.to_owned(), var);
        Ok(())
    }

    /// Set a variable in a specific local scope (the innermost is `0`).
    pub fn set_local(&mut self, index: usize, name: &str, var: EnvVar) -> Result<(), EnvSetError> {
        let scope = self
            .locals
            .get_mut(index)
            .ok_or_else(|| EnvSetError::Invalid(name.to_owned()))?;
        scope.vars.insert(name.to_owned(), var);
        Ok(())
    }

    /// Remove a variable from the innermost visible scope.
    ///
    /// Returns `true` if a binding was removed.
    pub fn remove(&mut self, name: &str) -> bool {
        for scope in self.locals.iter_mut().rev() {
            if scope.vars.remove(name).is_some() {
                return true;
            }
        }
        self.globals.vars.remove(name).is_some()
    }

    /// The environment to pass to a child process: every exported variable,
    /// with the innermost visible binding winning.
    pub fn exported_env(&self) -> Vec<(String, String)> {
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        for scope in self.locals.iter().rev().chain(core::iter::once(&self.globals)) {
            for (name, var) in &scope.vars {
                if var.is_exported() {
                    map.insert(name.clone(), var.as_env_string());
                }
            }
        }
        map.into_iter().collect()
    }

    fn top_scope_mut(&mut self) -> &mut Scope {
        if let Some(scope) = self.locals.last_mut() {
            scope
        } else {
            &mut self.globals
        }
    }
}

impl Default for EnvStack {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Display for EnvStack {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "<env stack depth {}>", self.depth())
    }
}

// The path separator is provided by the shell driver (std), which splits
// `PATH` using the platform's convention before seeding the stack.
