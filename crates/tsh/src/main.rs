//! # tsh — Token Shell
//!
//! A POSIX-compatible shell (brush-core) where every byte of final output
//! flows through a Rust router before reaching the terminal.
//!
//! ## Architecture
//!
//! ```text
//! brush-core (fd 1) ──→ pipe ──→ stdout router ──→ safety filter ──→ terminal
//! brush-core (fd 2) ──→ pipe ──→ stderr router ──→ terminal (direct)
//! internal pipes (cmd1|cmd2) ──→ untouched, OS speed
//! ```

mod session_tracker;

use anyhow::{Context, Result};
use brush_core::openfiles::OpenFile;
use clap::Parser;
use std::collections::HashMap;
use std::io::{self, IsTerminal, Read, Write};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

const MAX_DISPLAY_BYTES: usize = 1024 * 1024; // 1 MB

// ---------------------------------------------------------------------------
// Platform: PipeReader → tokio async file
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn pipe_reader_to_async(reader: std::io::PipeReader) -> tokio::fs::File {
    use std::os::unix::io::{FromRawFd, IntoRawFd};
    let raw = reader.into_raw_fd();
    let std_file = unsafe { std::fs::File::from_raw_fd(raw) };
    tokio::fs::File::from_std(std_file)
}

#[cfg(windows)]
fn pipe_reader_to_async(reader: std::io::PipeReader) -> tokio::fs::File {
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};
    let raw = reader.into_raw_handle();
    let std_file = unsafe { std::fs::File::from_raw_handle(raw) };
    tokio::fs::File::from_std(std_file)
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

/// Token Shell (tsh) — a compliance-instrumented POSIX shell.
///
/// Every command's output flows through a routing layer that can
/// inspect, filter, and redact data before it reaches the terminal.
#[derive(Parser, Debug, Clone)]
#[command(author, version, about)]
struct Args {
    /// Execute a command string and exit (like bash -c)
    #[arg(short = 'c', long = "command")]
    command: Option<String>,

    /// Disable the safety filter (pass-through mode for debugging)
    #[arg(long)]
    no_safety: bool,
}

// ---------------------------------------------------------------------------
// Safety filter subprocess lifecycle
// ---------------------------------------------------------------------------

struct SafetyProcess {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

/// Spawns the Python safety filter as a long-running subprocess.
/// Returns None if the safety filter is disabled or the script can't be found.
fn spawn_safety_filter() -> Option<SafetyProcess> {
    // Resolve the filter script. Same logic as langextract-host's resolve_shim_command:
    // check adjacent to executable first, then fall back to python/ in workspace.
    let filter_path = resolve_safety_filter_path()?;

    let python = resolve_python();
    let mut child = Command::new(&python)
        .arg(&filter_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit()) // safety filter diagnostics go straight to terminal
        .kill_on_drop(true)
        .spawn()
        .ok()?;

    let stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    Some(SafetyProcess {
        child,
        stdin,
        stdout,
    })
}

/// Finds the safety filter script. Checks:
/// 1. Adjacent to the tsh executable (production install)
/// 2. python/safety_filter.py relative to CWD (development)
fn resolve_safety_filter_path() -> Option<String> {
    if let Ok(mut exe_path) = std::env::current_exe() {
        exe_path.pop();
        let adjacent = exe_path.join("safety_filter.py");
        if adjacent.exists() {
            return Some(adjacent.to_string_lossy().to_string());
        }
    }

    let dev_path = std::path::Path::new("python/safety_filter.py");
    if dev_path.exists() {
        return Some(dev_path.to_string_lossy().to_string());
    }

    None
}

/// Resolves the Python interpreter. Prefers the uv venv if it exists.
fn resolve_python() -> String {
    // Check for uv venv in workspace
    let venv_python = if cfg!(windows) {
        ".venv/Scripts/python.exe"
    } else {
        ".venv/bin/python3"
    };
    if std::path::Path::new(venv_python).exists() {
        return venv_python.to_string();
    }
    // Fallback to system python
    "python3".to_string()
}

// ---------------------------------------------------------------------------
// Stdout router (with safety filter)
// ---------------------------------------------------------------------------

/// Routes stdout through the safety filter.
/// Binary data and stderr bypass the safety filter and go directly to terminal.
/// The tracker is checked before each command's output to inject read metadata.
async fn run_stdout_router_with_safety(
    mut pipe_reader: tokio::fs::File,
    mut safety_stdin: tokio::process::ChildStdin,
    mut safety_stdout: tokio::process::ChildStdout,
    tracker: Arc<session_tracker::SessionTracker>,
) {
    let mut buf = [0u8; 8192];
    let mut total_written: usize = 0;
    let mut truncation_warned = false;
    let mut first_chunk = true;
    let mut is_binary = false;

    // Task: read sanitized output from safety filter and write to terminal
    let consumer = tokio::spawn(async move {
        let mut out_buf = [0u8; 8192];
        let mut stdout = tokio::io::stdout();
        loop {
            match safety_stdout.read(&mut out_buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let _ = stdout.write_all(&out_buf[..n]).await;
                    let _ = stdout.flush().await;
                }
                Err(_) => break,
            }
        }
    });

    // Track the last command turn we've seen, so we detect command boundaries.
    let mut last_seen_turn: u32 = 0;

    // Main loop: read from shell pipe, route to safety filter or terminal
    loop {
        match pipe_reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let chunk = &buf[..n];

                if first_chunk {
                    if chunk.contains(&0u8) {
                        is_binary = true;
                    }
                    first_chunk = false;
                }

                // Check if a new command has started.
                if !is_binary {
                    let snapshot = tracker.current_snapshot();
                    if snapshot.turn != last_seen_turn {
                        last_seen_turn = snapshot.turn;

                        let _ = safety_stdin.write_all(b"[tsh:new-command]\n").await;

                        if let Some(current_read) = snapshot.current_read.filter(|r| r.is_repeat) {
                            let header = format!(
                                "[tsh:repeat-read path={} count={}]\n",
                                current_read.path.display(),
                                current_read.read_count
                            );
                            let _ = safety_stdin.write_all(header.as_bytes()).await;
                        }
                    }
                }

                if total_written >= MAX_DISPLAY_BYTES {
                    continue; // drain silently
                }

                let remaining = MAX_DISPLAY_BYTES - total_written;
                let bytes_to_write = n.min(remaining);
                let write_chunk = &chunk[..bytes_to_write];

                if is_binary {
                    // Binary bypass: skip safety filter, write directly to terminal
                    let _ = tokio::io::stdout().write_all(write_chunk).await;
                    let _ = tokio::io::stdout().flush().await;
                } else {
                    // Route text through safety filter
                    let _ = safety_stdin.write_all(write_chunk).await;
                    let _ = safety_stdin.flush().await;
                }

                total_written += bytes_to_write;

                if total_written >= MAX_DISPLAY_BYTES && !truncation_warned {
                    let _ = tokio::io::stdout()
                        .write_all(b"\n[STDOUT TRUNCATED at 1MB]\n")
                        .await;
                    truncation_warned = true;
                }
            }
            Err(_) => break,
        }
    }

    // Close safety filter stdin so the Python process sees EOF and flushes
    drop(safety_stdin);
    // Wait for the consumer to finish reading safety filter output
    let _ = consumer.await;
}

/// Routes output directly to terminal (no safety filter). Used for stderr,
/// or when --no-safety is set for stdout too.
async fn run_passthrough_router(
    mut pipe_reader: tokio::fs::File,
    mut terminal_writer: impl AsyncWriteExt + Unpin,
    label: &'static str,
) {
    let mut buf = [0u8; 8192];
    let mut total_written: usize = 0;
    let mut truncation_warned = false;

    loop {
        match pipe_reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                if total_written >= MAX_DISPLAY_BYTES {
                    continue;
                }

                let remaining = MAX_DISPLAY_BYTES - total_written;
                let bytes_to_write = n.min(remaining);

                let _ = terminal_writer.write_all(&buf[..bytes_to_write]).await;
                let _ = terminal_writer.flush().await;
                total_written += bytes_to_write;

                if total_written >= MAX_DISPLAY_BYTES && !truncation_warned {
                    let _ = terminal_writer
                        .write_all(format!("\n[{label} TRUNCATED at 1MB]\n").as_bytes())
                        .await;
                    truncation_warned = true;
                }
            }
            Err(_) => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Shell creation with fd injection
// ---------------------------------------------------------------------------

async fn create_instrumented_shell(
    interactive: bool,
    stdout_writer: std::io::PipeWriter,
    stderr_writer: std::io::PipeWriter,
    _observer: Arc<session_tracker::SessionTracker>,
) -> Result<brush_core::Shell> {
    let builtins = brush_builtins::default_builtins(brush_builtins::BuiltinSet::BashMode);

    let mut fds = HashMap::new();
    fds.insert(1, OpenFile::PipeWriter(stdout_writer));
    fds.insert(2, OpenFile::PipeWriter(stderr_writer));

    let create_options = brush_core::CreateOptions {
        interactive,
        shell_name: Some("tsh".to_string()),
        shell_product_display_str: Some(format!("tsh {}", env!("CARGO_PKG_VERSION"))),
        shell_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        read_commands_from_stdin: interactive,
        builtins,
        fds: Some(fds),
        ..Default::default()
    };

    let shell = brush_core::Shell::new(create_options)
        .await
        .context("Failed to create shell")?;

    Ok(shell)
}

// ---------------------------------------------------------------------------
// Execution modes
// ---------------------------------------------------------------------------

/// Sets up pipes, routers, and optionally the safety filter subprocess, then runs a closure.
async fn run_with_routing<F, Fut>(no_safety: bool, run_shell: F) -> Result<u8>
where
    F: FnOnce(brush_core::Shell, Arc<session_tracker::SessionTracker>) -> Fut,
    Fut: std::future::Future<Output = Result<(brush_core::Shell, u8)>>,
{
    let (stdout_reader, stdout_writer) = std::io::pipe()?;
    let (stderr_reader, stderr_writer) = std::io::pipe()?;

    let tracker = Arc::new(session_tracker::SessionTracker::new());
    let shell =
        create_instrumented_shell(false, stdout_writer, stderr_writer, tracker.clone()).await?;

    let async_stdout = pipe_reader_to_async(stdout_reader);
    let async_stderr = pipe_reader_to_async(stderr_reader);

    let stderr_router = tokio::spawn(run_passthrough_router(
        async_stderr,
        tokio::io::stderr(),
        "STDERR",
    ));

    let safety = if no_safety {
        None
    } else {
        spawn_safety_filter()
    };

    let stdout_router = if let Some(safety_proc) = safety {
        let t = tracker.clone();
        tokio::spawn(async move {
            run_stdout_router_with_safety(async_stdout, safety_proc.stdin, safety_proc.stdout, t)
                .await;
            let mut child = safety_proc.child;
            let _ = child.wait().await;
        })
    } else {
        tokio::spawn(run_passthrough_router(
            async_stdout,
            tokio::io::stdout(),
            "STDOUT",
        ))
    };

    // Run the shell
    let (shell, exit_code) = run_shell(shell, tracker.clone()).await?;

    // Close pipes
    drop(shell);

    let _ = stdout_router.await;
    let _ = stderr_router.await;

    Ok(exit_code)
}

/// -c mode
async fn run_command_mode(command: &str, no_safety: bool) -> Result<u8> {
    run_with_routing(no_safety, |mut shell, tracker| async move {
        let cwd = std::env::current_dir()?;
        let code = if let Some(segments) = split_repeat_read_segments(command, &cwd) {
            run_command_segments(&mut shell, &tracker, &segments).await?
        } else {
            tracker.on_command_start(command, &cwd);
            let params = shell.default_exec_params();
            shell
                .run_string(command, &params)
                .await
                .context("Command execution failed")?
                .exit_code
                .into()
        };
        Ok((shell, code))
    })
    .await
}

/// Interactive REPL mode
async fn run_interactive_mode(no_safety: bool) -> Result<u8> {
    let (stdout_reader, stdout_writer) = std::io::pipe()?;
    let (stderr_reader, stderr_writer) = std::io::pipe()?;

    let tracker = Arc::new(session_tracker::SessionTracker::new());
    let mut shell =
        create_instrumented_shell(true, stdout_writer, stderr_writer, tracker.clone()).await?;

    let async_stdout = pipe_reader_to_async(stdout_reader);
    let async_stderr = pipe_reader_to_async(stderr_reader);

    let stderr_router = tokio::spawn(run_passthrough_router(
        async_stderr,
        tokio::io::stderr(),
        "STDERR",
    ));

    let safety = if no_safety {
        None
    } else {
        spawn_safety_filter()
    };

    let stdout_router = if let Some(safety_proc) = safety {
        let t = tracker.clone();
        tokio::spawn(async move {
            run_stdout_router_with_safety(async_stdout, safety_proc.stdin, safety_proc.stdout, t)
                .await;
            let mut child = safety_proc.child;
            let _ = child.wait().await;
        })
    } else {
        tokio::spawn(run_passthrough_router(
            async_stdout,
            tokio::io::stdout(),
            "STDOUT",
        ))
    };

    // REPL: prompt goes to real terminal, command output goes through pipes
    let mut line_buf = String::new();
    loop {
        eprint!("tsh$ ");
        let _ = io::stderr().flush();

        line_buf.clear();
        match io::stdin().read_line(&mut line_buf) {
            Ok(0) => break,
            Ok(_) => {
                let input = line_buf.trim();
                if input.is_empty() {
                    continue;
                }
                if input == "exit" || input == "quit" {
                    break;
                }

                tracker.on_command_start(input, &std::env::current_dir()?);
                let params = shell.default_exec_params();
                if let Err(e) = shell.run_string(input, &params).await {
                    eprintln!("tsh: error: {}", e);
                }
            }
            Err(_) => break,
        }
    }

    let exit_code = shell.last_result();
    drop(shell);

    let _ = stdout_router.await;
    let _ = stderr_router.await;

    Ok(exit_code)
}

/// Piped stdin mode
async fn run_piped_mode(no_safety: bool) -> Result<u8> {
    let mut raw = Vec::new();
    io::stdin().read_to_end(&mut raw)?;
    if raw.is_empty() {
        anyhow::bail!("No input via stdin.");
    }
    let script = decode_stdin_bytes(raw)?;
    if script.trim().is_empty() {
        anyhow::bail!("No input via stdin.");
    }
    run_command_mode(&script, no_safety).await
}

// ---------------------------------------------------------------------------
// PowerShell encoding
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn decode_stdin_bytes(raw: Vec<u8>) -> Result<String> {
    if raw.len() >= 2 && raw[0] == 0xFF && raw[1] == 0xFE {
        let (d, _, e) = encoding_rs::UTF_16LE.decode(&raw[2..]);
        if e {
            anyhow::bail!("Failed to decode UTF-16LE from PowerShell.");
        }
        return Ok(d.into_owned());
    }
    if raw.len() >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF {
        return String::from_utf8(raw[3..].to_vec()).context("Invalid UTF-8 after BOM");
    }
    if let Ok(s) = String::from_utf8(raw.clone()) {
        return Ok(s);
    }
    let (d, _, e) = encoding_rs::UTF_16LE.decode(&raw);
    if e {
        anyhow::bail!("Not valid UTF-8 or UTF-16LE.");
    }
    Ok(d.into_owned())
}

#[cfg(not(windows))]
fn decode_stdin_bytes(raw: Vec<u8>) -> Result<String> {
    if raw.len() >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF {
        return String::from_utf8(raw[3..].to_vec()).context("Invalid UTF-8 after BOM");
    }
    String::from_utf8(raw).context("Not valid UTF-8")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandOp {
    AndIf,
    OrIf,
    Seq,
}

#[derive(Debug)]
struct CommandSegment {
    op_before: Option<CommandOp>,
    command: String,
}

fn split_top_level_commands(command: &str) -> Option<Vec<CommandSegment>> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut single_quote = false;
    let mut double_quote = false;
    let mut pending_op = None;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !double_quote => {
                single_quote = !single_quote;
                current.push(ch);
            }
            '"' if !single_quote => {
                double_quote = !double_quote;
                current.push(ch);
            }
            '&' if !single_quote && !double_quote && chars.peek() == Some(&'&') => {
                chars.next();
                push_segment(&mut segments, &mut current, pending_op.take());
                pending_op = Some(CommandOp::AndIf);
            }
            '|' if !single_quote && !double_quote && chars.peek() == Some(&'|') => {
                chars.next();
                push_segment(&mut segments, &mut current, pending_op.take());
                pending_op = Some(CommandOp::OrIf);
            }
            ';' if !single_quote && !double_quote => {
                push_segment(&mut segments, &mut current, pending_op.take());
                pending_op = Some(CommandOp::Seq);
            }
            _ => current.push(ch),
        }
    }

    if single_quote || double_quote {
        return None;
    }

    let tail = current.trim();
    if !tail.is_empty() {
        segments.push(CommandSegment {
            op_before: pending_op,
            command: tail.to_string(),
        });
    }

    if segments.len() <= 1 {
        return None;
    }

    let mut normalized = Vec::with_capacity(segments.len());
    let mut next_op = None;
    for segment in segments {
        normalized.push(CommandSegment {
            op_before: next_op,
            command: segment.command,
        });
        next_op = segment.op_before;
    }

    Some(normalized)
}

fn split_repeat_read_segments(command: &str, cwd: &std::path::Path) -> Option<Vec<CommandSegment>> {
    let segments = split_top_level_commands(command)?;
    let read_segments = segments
        .iter()
        .filter(|segment| session_tracker::command_reads_file(&segment.command, cwd))
        .count();

    (read_segments >= 2).then_some(segments)
}

fn push_segment(
    segments: &mut Vec<CommandSegment>,
    current: &mut String,
    next_op: Option<CommandOp>,
) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        segments.push(CommandSegment {
            op_before: next_op,
            command: trimmed.to_string(),
        });
    }
    current.clear();
}

async fn run_command_segments(
    shell: &mut brush_core::Shell,
    tracker: &session_tracker::SessionTracker,
    segments: &[CommandSegment],
) -> Result<u8> {
    let cwd = std::env::current_dir()?;
    let mut last_exit_code = 0u8;

    for segment in segments {
        let should_run = match segment.op_before {
            None | Some(CommandOp::Seq) => true,
            Some(CommandOp::AndIf) => last_exit_code == 0,
            Some(CommandOp::OrIf) => last_exit_code != 0,
        };

        if !should_run {
            continue;
        }

        tracker.on_command_start(&segment.command, &cwd);
        let params = shell.default_exec_params();
        let result = shell
            .run_string(&segment.command, &params)
            .await
            .with_context(|| format!("Command execution failed: {}", segment.command))?;
        last_exit_code = result.exit_code.into();
    }

    Ok(last_exit_code)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let exit_code = if let Some(ref command) = args.command {
        run_command_mode(command, args.no_safety).await?
    } else if !io::stdin().is_terminal() {
        run_piped_mode(args.no_safety).await?
    } else {
        run_interactive_mode(args.no_safety).await?
    };

    std::process::exit(i32::from(exit_code));
}
