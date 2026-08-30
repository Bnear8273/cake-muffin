//! Differential tests: run a snippet through both `cake` and real `bash` and
//! require identical stdout and exit status.
//!
//! These only cover features that are implemented today. Broader coverage
//! (including not-yet-implemented features) lives in `tests/corpus/` and is
//! driven by the `corpus_differential` test in this directory.

use std::process::{Command, Output};

/// Path to the built `cake` binary (provided by cargo for integration tests).
const CAKE: &str = env!("CARGO_BIN_EXE_cake");

struct ShellResult {
    stdout: String,
    stderr: String,
    code: i32,
}

fn run(cmd: &str, args: &[&str], input: &str) -> ShellResult {
    let out: Output = Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .expect("failed to run");
    ShellResult {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code().unwrap_or(-1),
    }
}

fn cake(src: &str) -> ShellResult {
    run(CAKE, &["-c", src], "")
}

fn bash(src: &str) -> ShellResult {
    run("bash", &["--posix", "-c", src], "")
}

fn has_bash() -> bool {
    Command::new("bash")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Assert `cake` and `bash` agree on stdout and exit code for `src`.
fn assert_shell_eq(src: &str) {
    assert!(has_bash(), "bash not available");
    let c = cake(src);
    let b = bash(src);
    assert_eq!(
        c.stdout, b.stdout,
        "STDOUT differs for: {src:?}\n  bash: {:?}\n  cake: {:?}\n  bash-stderr: {:?}\n  cake-stderr: {:?}",
        b.stdout, c.stdout, b.stderr, c.stderr
    );
    assert_eq!(
        c.code, b.code,
        "EXIT differs for: {src:?} (stdout matched)\n  bash: {} ({:?})\n  cake: {} ({:?})",
        b.code, b.stdout, c.code, c.stdout
    );
}

macro_rules! shell_eq {
    ($($src:expr),+ $(,)?) => {
        $( assert_shell_eq($src); )+
    };
}

// ---------------------------------------------------------------------------
// Variables and quoting
// ---------------------------------------------------------------------------

#[test]
fn variables_and_quoting() {
    shell_eq!(
        "x=5; echo $x",
        "x=hello; echo \"$x\"",
        "x=hello; echo '$x'",
        "echo \"a\\nb\"",
        "echo a\\ b",
        "echo 'single quoted'",
        "x='a b'; echo \"[$x]\"",
        "x=; echo empty:[$x]",
    );
}

#[test]
fn special_parameters() {
    shell_eq!(
        "true; echo $?",
        "false; echo $?",
        "x=1; echo ${x}",
        "echo \"$#\"",
        "a=one b=two; echo $a $b",
    );
}

// ---------------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------------

#[test]
fn if_elif_else() {
    shell_eq!(
        "if true; then echo yes; fi",
        "if false; then echo no; fi",
        "if false; then echo no; elif true; then echo elif; else echo else; fi",
        "if [ 1 -eq 1 ]; then echo eq; fi",
        "if [[ 3 -gt 2 ]]; then echo gt; fi",
    );
}

#[test]
fn loops() {
    shell_eq!(
        "for i in a b c; do echo \"$i\"; done",
        "i=0; while [ $i -lt 3 ]; do echo \"$i\"; i=$((i+1)); done",
        "i=0; until [ $i -ge 2 ]; do echo u$i; i=$((i+1)); done",
    );
}

#[test]
fn case_statement() {
    shell_eq!(
        "x=banana; case $x in apple) echo red;; banana) echo yellow;; *) echo other;; esac",
        "x=grape; case $x in a|b) echo ab;; *) echo other;; esac",
        "x=foo; case $x in f*) echo star;; esac",
    );
}

#[test]
fn blocks_and_subshells() {
    shell_eq!(
        "{ echo a; echo b; }",
        "(echo subshell)",
        "x=1; (x=2; echo \"in:$x\"); echo \"out:$x\"",
    );
}

#[test]
fn functions() {
    shell_eq!(
        "f() { echo \"fn $1\"; }; f hello",
        "f() { echo hi; }; f; f",
        "greet() { echo \"$1 $2\"; }; greet hello world",
    );
}

// ---------------------------------------------------------------------------
// Pipelines and logic
// ---------------------------------------------------------------------------

#[test]
fn pipelines() {
    shell_eq!(
        "echo hi | tr a-z A-Z",
        "echo a b c | tr ' ' '\\n' | wc -l",
        "printf 'x\\ny\\n' | grep y",
        "cat /etc/hostname | wc -c",
    );
}

#[test]
fn and_or_not() {
    shell_eq!(
        "true && echo and",
        "false || echo or",
        "! false && echo neg",
        "true && true && echo all",
        "false && echo no || echo fallback",
    );
}

// ---------------------------------------------------------------------------
// Redirections
// ---------------------------------------------------------------------------

#[test]
fn redirections() {
    shell_eq!(
        "echo hi > /dev/null",
        "echo out 2>/dev/null",
        "echo err >&2 2>/dev/null",
        "cat <<EOF\nhello\nEOF\n",
        "cat <<< herestring",
        "echo a; echo b >&2 2>/dev/null; echo c",
    );
}

// ---------------------------------------------------------------------------
// Arithmetic and conditional
// ---------------------------------------------------------------------------

#[test]
fn arithmetic() {
    shell_eq!(
        "echo $((2+3))",
        "echo $((10/3))",
        "echo $((2*3+1))",
        "x=5; echo $((x*2))",
        "x=0; ((x+=5)); echo $x",
        "echo $((2**3))",
        "echo $((10%3))",
    );
}

#[test]
fn conditionals() {
    shell_eq!(
        "[[ -n hello ]] && echo nonempty",
        "[[ -z '' ]] && echo empty",
        "[[ abc == abc ]] && echo same",
        "[[ abc != xyz ]] && echo diff",
        "[[ 5 -lt 10 ]] && echo lt",
        "[[ -d /tmp ]] && echo isdir",
        "[[ -e /etc/hostname ]] && echo exists",
        "x=7; [[ $x -ge 7 ]] && echo ge",
    );
}

// ---------------------------------------------------------------------------
// Builtins
// ---------------------------------------------------------------------------

#[test]
fn builtins() {
    shell_eq!(
        "echo -n hello",
        "printf 'x=%d\\n' 5",
        "x=1; unset x; echo \"[${x-}]\"; echo done",
        "export FOO=bar; echo $FOO",
        "true; echo ok",
        "pwd | grep -q / && echo has-slash",
    );
}

#[test]
fn assignments_are_not_temporary() {
    shell_eq!("A=1; echo $A; A=2; echo $A", "x=1 y=2; echo $x $y",);
}
