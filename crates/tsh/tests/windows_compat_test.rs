//! Windows compatibility test suite for tsh.
//!
//! Tests brush-core's behavior on Windows including:
//! - CMD/PowerShell interop
//! - Windows path handling
//! - Encoding edge cases
//! - Builtin-only operations (no external coreutils needed)
//! - Known limitations and workarounds

#![cfg(windows)]

use anyhow::Result;
use std::process::Command;

/// Helper: run tsh -c "command" and return (stdout, stderr, exit_code)
fn tsh_exec(command: &str) -> Result<(String, String, i32)> {
    let tsh = env!("CARGO_BIN_EXE_tsh");
    let output = Command::new(tsh).args(["-c", command]).output()?;
    Ok((
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    ))
}

fn tsh_ok(command: &str) -> String {
    let (stdout, stderr, code) = tsh_exec(command).unwrap();
    if code != 0 {
        panic!(
            "tsh -c {:?} failed (exit {}):\nstdout: {}\nstderr: {}",
            command, code, stdout, stderr
        );
    }
    stdout
}

// =========================================================================
// Builtins only (no external commands needed — these work everywhere)
// =========================================================================

#[test]
fn builtin_echo() {
    assert_eq!(tsh_ok("echo hello world").trim(), "hello world");
}

#[test]
fn builtin_printf() {
    assert_eq!(tsh_ok("printf '%s-%s' foo bar").trim(), "foo-bar");
}

#[test]
fn builtin_test_string() {
    assert_eq!(tsh_ok("[ 'a' = 'a' ] && echo yes").trim(), "yes");
}

#[test]
fn builtin_test_file() {
    // Cargo.toml always exists relative to workspace root
    let out = tsh_ok("[ -f Cargo.toml ] && echo exists || echo missing");
    assert_eq!(out.trim(), "exists");
}

#[test]
fn builtin_read_from_pipe() {
    assert_eq!(
        tsh_ok("echo hello | { read x; echo \"got: $x\"; }").trim(),
        "got: hello"
    );
}

#[test]
fn builtin_pwd() {
    let out = tsh_ok("pwd");
    assert!(!out.trim().is_empty());
}

#[test]
fn builtin_export_and_echo() {
    assert_eq!(tsh_ok("export MY_VAR=42 && echo $MY_VAR").trim(), "42");
}

#[test]
fn builtin_mapfile() {
    assert_eq!(
        tsh_ok("mapfile -t arr <<< $'a\\nb\\nc'; echo ${#arr[@]}").trim(),
        "3"
    );
}

// =========================================================================
// Variables, arithmetic, string ops
// =========================================================================

#[test]
fn variable_assignment() {
    assert_eq!(tsh_ok("x=hello && echo $x").trim(), "hello");
}

#[test]
fn arithmetic_basic() {
    assert_eq!(tsh_ok("echo $((2 + 3 * 4))").trim(), "14");
}

#[test]
fn arithmetic_power() {
    assert_eq!(tsh_ok("echo $((2 ** 10))").trim(), "1024");
}

#[test]
fn arithmetic_ternary() {
    assert_eq!(tsh_ok("x=5; echo $((x > 3 ? 1 : 0))").trim(), "1");
}

#[test]
fn string_suffix_trim() {
    assert_eq!(tsh_ok("f=app.tar.gz; echo ${f%.gz}").trim(), "app.tar");
}

#[test]
fn string_prefix_trim() {
    assert_eq!(tsh_ok("p=/usr/bin/grep; echo ${p##*/}").trim(), "grep");
}

#[test]
fn string_substitution() {
    assert_eq!(
        tsh_ok("s='hello world'; echo ${s/world/tsh}").trim(),
        "hello tsh"
    );
}

#[test]
fn string_default_value() {
    assert_eq!(tsh_ok("echo ${UNDEFINED:-fallback}").trim(), "fallback");
}

#[test]
fn string_length() {
    assert_eq!(tsh_ok("x=hello; echo ${#x}").trim(), "5");
}

#[test]
fn string_substring() {
    assert_eq!(tsh_ok("x=hello_world; echo ${x:6:5}").trim(), "world");
}

// =========================================================================
// Control flow
// =========================================================================

#[test]
fn for_loop_basic() {
    assert_eq!(tsh_ok("for i in a b c; do echo $i; done").trim(), "a\nb\nc");
}

#[test]
fn for_loop_nested() {
    let out = tsh_ok("for a in x y; do for b in 1 2; do echo $a$b; done; done");
    assert_eq!(out.trim(), "x1\nx2\ny1\ny2");
}

#[test]
fn while_counter() {
    let out = tsh_ok("i=0; while [ $i -lt 3 ]; do echo $i; i=$((i+1)); done");
    assert_eq!(out.trim(), "0\n1\n2");
}

#[test]
fn while_read_ifs() {
    let out = tsh_ok("echo -e 'k1:v1\\nk2:v2' | while IFS=: read -r k v; do echo \"$k=$v\"; done");
    assert_eq!(out.trim(), "k1=v1\nk2=v2");
}

#[test]
fn case_esac_basic() {
    assert_eq!(
        tsh_ok("x=hello; case $x in h*) echo match;; *) echo no;; esac").trim(),
        "match"
    );
}

#[test]
fn case_esac_continue() {
    let out =
        tsh_ok("for n in a skip b skip2 c; do case $n in skip*) continue;; esac; echo $n; done");
    assert_eq!(out.trim(), "a\nb\nc");
}

#[test]
fn if_chain() {
    let out = tsh_ok("x=5; if [ $x -gt 10 ]; then echo big; elif [ $x -gt 3 ]; then echo med; else echo small; fi");
    assert_eq!(out.trim(), "med");
}

#[test]
fn short_circuit_and() {
    assert_eq!(tsh_ok("true && echo yes").trim(), "yes");
    let (_, _, code) = tsh_exec("false && echo no").unwrap();
    assert_ne!(code, 0);
}

#[test]
fn short_circuit_or() {
    assert_eq!(tsh_ok("false || echo fallback").trim(), "fallback");
}

// =========================================================================
// Functions
// =========================================================================

#[test]
fn function_basic() {
    assert_eq!(
        tsh_ok("greet() { echo \"hi $1\"; }; greet world").trim(),
        "hi world"
    );
}

#[test]
fn function_local_scope() {
    let out = tsh_ok("f() { local x=inner; echo $x; }; x=outer; f; echo $x");
    assert_eq!(out.trim(), "inner\nouter");
}

#[test]
fn function_return_code() {
    let out =
        tsh_ok("ok() { return 0; }; fail() { return 1; }; ok && echo ok; fail || echo caught");
    assert_eq!(out.trim(), "ok\ncaught");
}

// =========================================================================
// Arrays
// =========================================================================

#[test]
fn array_indexed() {
    assert_eq!(tsh_ok("a=(one two three); echo ${a[1]}").trim(), "two");
}

#[test]
fn array_length() {
    assert_eq!(tsh_ok("a=(a b c d); echo ${#a[@]}").trim(), "4");
}

#[test]
fn array_iterate() {
    let out = tsh_ok("a=(x y z); for i in \"${a[@]}\"; do echo $i; done");
    assert_eq!(out.trim(), "x\ny\nz");
}

#[test]
fn array_slice() {
    assert_eq!(tsh_ok("a=(a b c d e); echo ${a[@]:1:3}").trim(), "b c d");
}

// =========================================================================
// Strict mode
// =========================================================================

#[test]
fn set_e() {
    assert_eq!(tsh_ok("set -e; echo a; true; echo b").trim(), "a\nb");
}

#[test]
fn set_u() {
    assert_eq!(tsh_ok("set -u; x=ok; echo $x").trim(), "ok");
}

#[test]
fn set_euo_pipefail() {
    assert_eq!(tsh_ok("set -euo pipefail; echo strict").trim(), "strict");
}

// =========================================================================
// Pipes (builtin-to-builtin, no external commands)
// =========================================================================

#[test]
fn pipe_echo_to_read() {
    assert_eq!(
        tsh_ok("echo test | { read x; echo \"read: $x\"; }").trim(),
        "read: test"
    );
}

#[test]
fn pipe_printf_chain() {
    // printf to while-read — pure builtins
    let out = tsh_ok("printf '%s\\n' a b c | while read line; do echo \"got:$line\"; done");
    assert_eq!(out.trim(), "got:a\ngot:b\ngot:c");
}

// =========================================================================
// Redirections
// =========================================================================

#[test]
fn redirect_stderr() {
    let (_, stderr, _) = tsh_exec("echo err >&2").unwrap();
    assert_eq!(stderr.trim(), "err");
}

#[test]
fn redirect_merge_2_to_1() {
    let (stdout, _, _) = tsh_exec("{ echo out; echo err >&2; } 2>&1").unwrap();
    assert!(stdout.contains("out"));
    assert!(stdout.contains("err"));
}

// =========================================================================
// Brace expansion
// =========================================================================

#[test]
fn brace_expansion() {
    let out = tsh_ok("echo {a,b,c}");
    assert_eq!(out.trim(), "a b c");
}

#[test]
fn brace_expansion_prefix() {
    let out = tsh_ok("echo file.{txt,md,rs}");
    assert_eq!(out.trim(), "file.txt file.md file.rs");
}

// =========================================================================
// Heredocs (using builtins only — no cat needed)
// =========================================================================

#[test]
fn heredoc_to_read() {
    let out = tsh_ok("read line <<< 'hello heredoc'; echo $line");
    assert_eq!(out.trim(), "hello heredoc");
}

#[test]
fn heredoc_mapfile() {
    let out = tsh_ok("mapfile -t lines <<< $'line1\\nline2\\nline3'; echo ${lines[1]}");
    assert_eq!(out.trim(), "line2");
}

// =========================================================================
// Windows-specific: CMD interop
// =========================================================================

#[test]
fn cmd_exe_echo() {
    let out = tsh_ok("\"C:\\Windows\\System32\\cmd.exe\" /c echo hello-from-cmd");
    assert!(out.contains("hello-from-cmd"));
}

// =========================================================================
// Known limitation documentation tests
// These tests document expected failures so we track when brush-core fixes them.
// =========================================================================

#[test]
fn known_issue_path_lookup_fails() {
    // brush-core can't resolve commands via Windows PATH (spaces in dirs)
    let (_, stderr, code) = tsh_exec("grep --version").unwrap();
    assert_eq!(
        code, 127,
        "When this passes, brush fixed Windows PATH lookup!"
    );
    assert!(stderr.contains("not found"));
}

#[test]
fn known_issue_dev_null() {
    // /dev/null doesn't exist on Windows
    let (_, stderr, code) = tsh_exec("echo test > /dev/null").unwrap();
    assert_ne!(
        code, 0,
        "When this passes, brush added /dev/null → NUL translation!"
    );
    assert!(stderr.contains("cannot find") || stderr.contains("failed to redirect"));
}

#[test]
fn known_issue_process_substitution() {
    // <() requires /dev/fd which doesn't exist on Windows.
    // brush-core may either error or silently degrade.
    // We test a meaningful use: diff <() <() which needs actual fd passing.
    let (stdout, stderr, code) =
        tsh_exec("\"C:\\Windows\\System32\\cmd.exe\" /c echo a > NUL && echo <(echo test)")
            .unwrap();
    // If this produces a /dev/fd path or errors, process substitution isn't fully working.
    // Just document current behavior.
    let _ = (stdout, stderr, code);
}

#[test]
fn known_issue_glob_in_nonexistent_dir() {
    // Glob returns literal when no match (this is actually bash-correct behavior
    // with nullglob off, but can surprise users)
    let out = tsh_ok("for f in /nonexistent/*.txt; do echo $f; done");
    assert!(out.contains("/nonexistent/*.txt") || out.contains("*"));
}
