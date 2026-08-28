# cake

A bash-compatible shell with a modern interactive layer, written in Rust.

`cake` aims to be a drop-in-ish replacement for `bash` for everyday interactive
use: the full control-flow / pipeline / expansion semantics of bash, wrapped in
a modern line editor with syntax highlighting, tab completion, context-aware
autosuggestion and a customizable prompt.

## Features

- **Control flow**: `if`/`elif`/`else`, `for`, `while`, `until`, `case`, `{ }`
  blocks, `( )` subshells, functions.
- **Pipelines**: `|`, `|&`, `&&`, `||`, `!`, background `&`.
- **Redirections**: `>`, `>>`, `<`, `2>`, `2>&1`, `&>`, `&>>`, heredocs,
  herestrings.
- **Expansion** (bash order): brace expansion, tilde, parameter expansion
  (`$var`, `${var}`, `${#x}`, `${x:0:5}`, `${x/pat/rep}`, `${x^^}`, `${x:-d}`,
  `${x#pat}`, ...), indirect expansion (`${!name}`), shell-quoted escape
  (`${var@Q}`), command substitution `$(...)` / backticks, arithmetic
  `$((...))` / `((...))`, IFS field splitting, pathname (glob) expansion,
  quote removal.
- **Conditionals**: `[ ... ]` and `[[ ... ]]` tests, including `[[ =~ ]]`
  regex matching.
- **Builtins**: `echo`, `printf`, `cd`, `pwd`, `export`, `unset`, `readonly`,
  `local`, `declare`/`typeset`, `shift`, `type`, `alias`, `unalias`, `exit`,
  `true`/`false`/`:`, `[`/`test`, `read`, `source`, `break`/`continue`/`return`,
  `set` (`-e`/`-u`/`-f`, `-o pipefail`), `shopt` (`nullglob`/`dotglob`/
  `nocaseglob`/`extglob`/`globstar`), `trap` (`EXIT`/`ERR`/signals), `pushd`,
  `popd`, `dirs`, `getopts`, `jobs`, `wait`, `fg`/`bg`, and more.
- **Shell options**: `set -e` errexit with bash's exemption contexts,
  `set -u` nounset, `set -o pipefail`.
- **Job control**: background jobs (`cmd &`), `$!`, `jobs`, `wait [pid]`.
- **Process substitution**: `<(cmd)` and `>(cmd)`.
- **extglob**: `?(p)` `*(p)` `+(p)` `@(p)` `!(p)` patterns in globbing,
  `case` and `[[ == ]]` (the latter always active, like bash).
- **globstar** (`shopt -s globstar`): `**` matches any number of directories
  recursively (off by default, like bash).
- **POSIX char classes** in globs: `[[:alpha:]]`, `[[:digit:]]`, `[[:alnum:]]`,
  `[[:upper:]]`, `[[:lower:]]`, `[[:space:]]`, `[[:punct:]]`, etc.
- **`$(<file)`** shorthand for reading a file.
- **Special variables**: `$RANDOM`, `$LINENO`, `$SECONDS`, `$PPID`,
  `$PWD`/`$OLDPWD`.
- **Interactive layer**:
  - syntax highlighting (found commands green, unknown red, keywords magenta)
  - tab completion (commands, files, variables)
  - context-aware autosuggestion from history
  - `~`-relative path prompt (override with `PS1`)
  - history + "command not found" blacklist persisted under the XDG data dir
  - a startup rc file at `$XDG_DATA_HOME/cake/rc` (aliases, `PS1`, ...) is
    evaluated at session start

## Usage

```console
$ cake                      # interactive shell
$ cake -c 'echo hello'      # run one command and exit
$ cake --help
```

Aliases work in interactive mode (like bash, aliases don't expand in `-c`):

```console
$ alias ls='ls --color=auto'
$ alias grep='grep --color=auto'
$ ls -la                    # runs: ls --color=auto -la
$ type ls
ls is an alias for ls --color=auto
```

> **Why colour aliases?** Tools that detect a tty themselves (`git`, `rg`,
> `bat`) colourise automatically. GNU coreutils tools (`ls`, `grep`, `diff`)
> need an explicit flag, which shells normally provide via aliases. Use
> `--color=auto` (colourise only on a tty) so piped output stays clean.

## Architecture

A Cargo workspace of 13 crates with a strict split between pure `#![no_std]`
shell logic and the OS-dependent layer:

```
cake                  the binary: arg parsing, REPL loop, platform wiring
crates/cake-syntax    lexer + recursive-descent parser + AST (no_std)
crates/cake-exec      evaluator: control flow, expansion, pipelines, builtins
crates/cake-env       copy-on-write EnvVar / EnvStack scoping
crates/cake-proc      job/process data model
crates/cake-platform  backend-neutral Platform trait (no_std)
crates/cake-platform-unix   real backend (nix/libc)
crates/cake-platform-mock   test backend
crates/cake-complete  tab-completion logic (no_std)
crates/cake-highlight syntax-highlighting logic (no_std)
crates/cake-reader    autosuggestion logic (no_std)
crates/cake-blacklist persistent "command not found" set (no_std)
crates/cake-builtin   reserved for a future builtin split (no_std)
```

Every pure-logic crate is `#![no_std]` (only `alloc`); only the binary and the
platform backends touch `std` / the OS. All OS access (fork/exec, waitpid,
signals, termios, filesystem, XDG dirs) goes through the global
`Platform` trait object, so the shell core is portable and testable in
isolation.

## Building & testing

```console
$ cargo build
$ cargo test              # unit tests + differential corpus (needs bash)
$ cargo clippy -- -D warnings
```

`cargo test` runs two differential suites that compare `cake` against real
`bash --posix` on stdout bytes and exit codes:

- `cake/tests/shell_compat.rs` — curated snippets.
- `cake/tests/corpus.rs` — drives every file in `tests/corpus/`; files are
  marked `# cake:xfail` (expected to differ) or `# cake:skip`.

## Status

The bash core (parser, evaluator, expansion) and the interactive layer are
implemented. Known gaps on the roadmap:

- terminal job control (`fg`/`bg` on a real tty, `Ctrl-Z`)
- `history` expansion, `disown`
- `BASH_ENV`/`ENV`-style script startup files
- `$BASH_ENV` for non-interactive startup (the rc file covers interactive)
- a Windows backend (`run_in_child` is `fork`-based today)

## License

Apache-2.0
