//! Tests for the session-level repeat-read tracking.
//!
//! Verifies that:
//! 1. First read of a large file shows structural skeleton (head + structural + tail)
//! 2. Second read of the SAME file in the same session shows structure-only
//! 3. The repeat read is dramatically smaller than the first read
//! 4. Critical needles (ERROR, WARN, def, class, etc.) survive both reads

use std::process::Command;

/// Run a tsh -c command from the workspace root, return stdout
fn tsh_run(cmd: &str) -> (String, i32) {
    let tsh = env!("CARGO_BIN_EXE_tsh");
    let output = Command::new(tsh)
        .args(["-c", cmd])
        .current_dir(env!("CARGO_MANIFEST_DIR").to_string() + "/../..")
        .output()
        .expect("failed to run tsh");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, code)
}

/// Helper to get the cat command path (platform-specific)
fn cat_cmd() -> &'static str {
    if cfg!(windows) {
        "\"C:/Program Files/Git/usr/bin/cat.exe\""
    } else {
        "cat"
    }
}

// =========================================================================
// First-read tests (structural skeleton)
// =========================================================================

#[test]
fn first_read_large_file_is_limited() {
    let cat = cat_cmd();
    let (out, _) = tsh_run(&format!("{cat} tests/jungle/server_log.txt"));

    let out_lines = out.lines().count();
    // Raw file is 2000 lines, should be limited to ~141
    assert!(
        out_lines < 200,
        "First read should be limited, got {out_lines} lines"
    );
    assert!(out.contains("[tsh:"), "Should have tsh footer");
}

#[test]
fn first_read_preserves_errors() {
    let cat = cat_cmd();
    let (out, _) = tsh_run(&format!("{cat} tests/jungle/server_log.txt"));

    assert!(
        out.contains("[ERROR]"),
        "First read should preserve ERROR lines"
    );
    assert!(
        out.contains("[FATAL]"),
        "First read should preserve FATAL lines"
    );
}

#[test]
fn first_read_preserves_code_structure() {
    let cat = cat_cmd();
    let (out, _) = tsh_run(&format!("{cat} tests/jungle/large_python.py"));

    assert!(out.contains("class "), "Should preserve class definitions");
    assert!(out.contains("def "), "Should preserve function definitions");
    assert!(out.contains("import "), "Should preserve imports");
}

// =========================================================================
// Repeat-read tests (structure-only mode)
// =========================================================================

#[test]
fn repeat_read_is_smaller_than_first() {
    let cat = cat_cmd();
    // Two reads in one session — split on the repeat-read announcement
    let cmd = format!("{cat} tests/jungle/server_log.txt && {cat} tests/jungle/server_log.txt");
    let (out, _) = tsh_run(&cmd);

    // The output should contain "repeat read #2" — split there
    if let Some(pos) = out.find("repeat read #2") {
        let first_section = &out[..pos];
        let second_section = &out[pos..];

        let first_lines = first_section.lines().count();
        let second_lines = second_section.lines().count();

        assert!(
            second_lines < first_lines,
            "Repeat section ({second_lines} lines) should be smaller than first ({first_lines} lines)"
        );
    } else {
        panic!("No 'repeat read #2' found in output");
    }
}

#[test]
fn repeat_read_announces_itself() {
    let cat = cat_cmd();
    let cmd = format!("{cat} tests/jungle/server_log.txt && {cat} tests/jungle/server_log.txt");
    let (out, _) = tsh_run(&cmd);

    assert!(
        out.contains("repeat read #2"),
        "Second read should announce itself as repeat read #2. Output:\n{}",
        &out[out.len().saturating_sub(500)..]
    );
}

#[test]
fn repeat_read_still_shows_errors() {
    let cat = cat_cmd();
    let cmd = format!("{cat} tests/jungle/server_log.txt && {cat} tests/jungle/server_log.txt");
    let (out, _) = tsh_run(&cmd);

    // After "repeat read" marker, ERROR and FATAL should still be present
    if let Some(repeat_pos) = out.find("repeat read #2") {
        let repeat_section = &out[repeat_pos..];
        assert!(
            repeat_section.contains("[ERROR]"),
            "Repeat read should still show ERROR lines"
        );
        assert!(
            repeat_section.contains("[FATAL]"),
            "Repeat read should still show FATAL lines"
        );
    } else {
        panic!("No repeat read #2 found in output");
    }
}

#[test]
fn repeat_read_code_preserves_structure() {
    let cat = cat_cmd();
    let cmd = format!("{cat} tests/jungle/large_python.py && {cat} tests/jungle/large_python.py");
    let (out, _) = tsh_run(&cmd);

    if let Some(repeat_pos) = out.find("repeat read #2") {
        let repeat_section = &out[repeat_pos..];
        assert!(
            repeat_section.contains("class ") || repeat_section.contains("def "),
            "Repeat read should preserve code structure"
        );
    } else {
        panic!("No repeat read found in output");
    }
}

#[test]
fn repeat_read_dramatic_reduction() {
    let cat = cat_cmd();
    let cmd = format!(
        "{cat} tests/jungle/server_log.txt && echo SPLIT && {cat} tests/jungle/server_log.txt"
    );
    let (out, _) = tsh_run(&cmd);

    let parts: Vec<&str> = out.split("SPLIT").collect();
    if parts.len() == 2 {
        let first = parts[0].lines().count();
        let second = parts[1].lines().count();

        // The repeat read should be at least 5x smaller
        assert!(
            second * 5 < first,
            "Repeat read ({second} lines) should be at least 5x smaller than first ({first} lines)"
        );
    }
}

// =========================================================================
// Third read — should be same as second (stable)
// =========================================================================

#[test]
fn third_read_same_as_second() {
    let cat = cat_cmd();
    let cmd = format!(
        "{cat} tests/jungle/server_log.txt && echo SPLIT1 && \
         {cat} tests/jungle/server_log.txt && echo SPLIT2 && \
         {cat} tests/jungle/server_log.txt"
    );
    let (out, _) = tsh_run(&cmd);

    let parts: Vec<&str> = out.split("SPLIT1").collect();
    if parts.len() >= 2 {
        let after_first: Vec<&str> = parts[1].split("SPLIT2").collect();
        if after_first.len() == 2 {
            let second_lines = after_first[0].lines().count();
            let third_lines = after_first[1].lines().count();

            // Second and third should be roughly the same size (both structure-only)
            let diff = (second_lines as i32 - third_lines as i32).unsigned_abs();
            assert!(
                diff < 5,
                "2nd ({second_lines}) and 3rd ({third_lines}) reads should be similar size"
            );
        }
    }
}
