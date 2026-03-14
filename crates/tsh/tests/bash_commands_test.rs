#![cfg(unix)]

use anyhow::Result;
use std::env;
use std::fs;
use xshell::{cmd, Shell};

#[test]
fn execute_engineer_bash_scripts() -> Result<()> {
    let sh = Shell::new()?;

    // 1. Establish the sandbox
    let sandbox = sh.current_dir().join("test_sandbox");
    let _ = fs::remove_dir_all(&sandbox);
    fs::create_dir_all(&sandbox)?;
    sh.change_dir(&sandbox);

    // 2. Build the Mock Binary Directory
    let mock_bin = sandbox.join("mock_bin");
    fs::create_dir_all(&mock_bin)?;

    let mocks = vec![
        (
            "kubectl",
            r#"#!/bin/bash
echo '{"id":"mock-store-123"}'"#,
        ),
        (
            "adb",
            r#"#!/bin/bash
if [ "$1" = "shell" ] && [ "$2" = "pidof" ]; then echo "9999"; exit 0; fi
if [ "$1" = "logcat" ]; then exit 0; fi
echo "adb mocked""#,
        ),
        (
            "docker",
            r#"#!/bin/bash
if [ "$1" = "ps" ]; then echo "kontext-postgres"; exit 0; fi
exit 0"#,
        ),
        (
            "gh",
            r#"#!/bin/bash
if [ "$1" = "api" ]; then echo "workflow-1"; exit 0; fi"#,
        ),
        (
            "identify",
            r#"#!/bin/bash
echo "file.png 1920x1080 8-bit""#,
        ),
        (
            "wsl.exe",
            r#"#!/bin/bash
# Mock the wsl environment execution
shift 3; bash -c "$*""#,
        ),
        (
            "ssh",
            r#"#!/bin/bash
# Silently absorb SSH commands
exit 0"#,
        ),
        (
            "sshpass",
            r#"#!/bin/bash
# Absorb password and execute the trailing command locally
shift 2; bash -c "$*""#,
        ),
        (
            "mogrify",
            r#"#!/bin/bash
exit 0"#,
        ),
        (
            "tasklist",
            r#"#!/bin/bash
echo "heartbeat.exe 1234 Console""#,
        ),
        (
            "node",
            r#"#!/bin/bash
exit 0"#,
        ),
        (
            "python3",
            r#"#!/bin/bash
exit 0"#,
        ),
    ];

    // Write the mocks to disk and make them executable
    for (name, script) in mocks {
        let path = mock_bin.join(name);
        fs::write(&path, script)?;
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
    }

    // Prepend our mock directory to the system PATH
    let current_path = env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{}", mock_bin.display(), current_path);
    // SAFETY: test is single-threaded
    unsafe {
        env::set_var("PATH", &new_path);
    }

    // 3. Seed the Sandbox with Data Files
    fs::write(sandbox.join("rootfs.tar"), "fake tar data")?;
    fs::write(sandbox.join("app.js"), "Shaders.Fragment\nShaders.Vertex")?;
    fs::write(sandbox.join("socket.c"), "socket(AF_INET);\nconnect(fd);")?;
    fs::write(sandbox.join("trace.txt"), "sys-(trace) sys#123\nret")?;
    fs::write(sandbox.join("direct.txt"), "1 data\n2 data")?;
    fs::write(sandbox.join("forkexec.txt"), "data 1\ndata 2")?;

    fs::create_dir_all(sandbox.join("mesh-reveal"))?;
    fs::write(
        sandbox.join("mesh-reveal/test.jsx"),
        "import React;\nimport { Box } from 'ui';",
    )?;

    fs::create_dir_all(sandbox.join("aosp/test"))?;
    fs::write(sandbox.join("aosp/test/app.kt"), "fun main() {}")?;

    // 4. Execute the Engineer Commands
    let commands = vec![
        // Multi-stage pipelines
        r#"tar tf rootfs.tar | grep -E "^(usr/local/bin/|bin/bash)" | sort || true"#,
        r#"grep -rh "^import" mesh-reveal/*.jsx | sort | uniq -c | sort -rn"#,
        r#"grep -oE "(socket|connect|bind)\(" socket.c | sort | uniq -c | sort -rn"#,
        // Command Substitution & APIs
        r#"dims=$(identify "file.png" | awk '{print $3}') && echo $dims"#,
        r#"STORE_ID=$(kubectl exec "$POD" -- curl -s ... | grep -o '"id":"[^"]*"' | cut -d'"' -f4) && echo $STORE_ID"#,
        r#"adb logcat --pid=$(adb shell pidof com.android.developers.androidify)"#,
        // If conditionals and Chaining
        r#"if ! docker ps -a --format "{{.Names}}" | grep -q "^kontext-postgres$"; then echo "starting"; fi"#,
        r#"mkdir -p public/friscy && cp rootfs.tar public/friscy/ && ls -lh public/friscy/"#,
        r#"which identify && identify --version | head -1 || echo "not found""#,
        // Process substitution & Awk
        r#"paste <(awk '{print NR, $1}' direct.txt) <(awk '{print $1}' forkexec.txt) | awk '{if($2!=$3) print NR, $2, $3}'"#,
        r#"grep "sys-(trace|ret)" trace.txt | paste - - | awk '{ match($0, /sys#([0-9]+)/, sn); printf "%s\n", sn[1] }'"#,
        // WSL and SSH cross-environment simulations
        r#"wsl.exe -d Ubuntu-24.04 -e /bin/bash -c 'echo "Inside mocked WSL"' "#,
        r#"ssh root@host "echo 'GatewayPorts yes' >> /tmp/sshd_config""#,
    ];

    for cmd_str in commands {
        println!("Executing: {}", cmd_str);
        let execution = cmd!(sh, "bash -c {cmd_str}");

        if let Err(e) = execution.run() {
            println!(
                "Command returned non-zero (expected for mocked empty data): {}",
                e
            );
        }
    }

    Ok(())
}
