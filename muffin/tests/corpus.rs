//! Differential corpus runner: run every `.sh` file in `tests/corpus/` through
//! both `muffin` and `bash --posix` and assert stdout + exit code agreement.
//!
//! Markers in a corpus file (first-line comments):
//!   `# muffin:xfail` — expected to differ (feature not yet implemented).
//!   `# muffin:skip`  — don't run this file.

use std::path::Path;
use std::process::{Command, Output};

const CAKE: &str = env!("CARGO_BIN_EXE_muffin");

struct ShellResult {
    stdout: String,
    code: i32,
}

fn run(cmd: &str, args: &[&str], input: &str) -> ShellResult {
    let out: Output = Command::new(cmd)
        .args(args)
        // A fixed locale keeps job-status words ("Running") identical.
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .expect("failed to run");
    ShellResult {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        code: out.status.code().unwrap_or(-1),
    }
}

fn muffin(src: &str) -> ShellResult {
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

#[test]
fn corpus_differential() {
    if !has_bash() {
        eprintln!("bash not found — skipping corpus tests");
        return;
    }

    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tests")
        .join("corpus");

    let mut entries: Vec<_> = std::fs::read_dir(&corpus_dir)
        .expect("corpus directory not found")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("sh"))
        .collect();
    entries.sort();

    let (mut pass, mut xfail, mut skip, mut fail) = (0usize, 0, 0, 0);
    let mut failures: Vec<String> = Vec::new();

    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy();
        let src = std::fs::read_to_string(path).unwrap_or_default();

        let marker = src
            .lines()
            .next()
            .and_then(|l| l.trim().strip_prefix("# muffin:"));

        match marker {
            Some("skip") => {
                skip += 1;
                eprintln!("SKIP  {name}");
                continue;
            }
            Some("xfail") => {
                let b = bash(&src);
                let c = muffin(&src);
                if b.stdout == c.stdout && b.code == c.code {
                    // Now passing — promote to pass.
                    xfail += 1;
                    eprintln!("XFIX  {name} (now passing)");
                } else {
                    xfail += 1;
                    eprintln!("XFAIL {name}");
                }
            }
            _ => {
                let b = bash(&src);
                let c = muffin(&src);
                if b.stdout == c.stdout && b.code == c.code {
                    pass += 1;
                    eprintln!("PASS  {name}");
                } else {
                    fail += 1;
                    failures.push(name.to_string());
                    eprintln!("FAIL  {name}");
                }
            }
        }
    }

    eprintln!();
    eprintln!("==== corpus summary ====");
    eprintln!("PASS  {pass}");
    eprintln!("XFAIL {xfail}");
    eprintln!("FAIL  {fail}");
    eprintln!("SKIP  {skip}");

    if !failures.is_empty() {
        panic!("corpus failures: {failures:?}");
    }
}
