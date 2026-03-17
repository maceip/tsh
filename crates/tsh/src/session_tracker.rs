//! Session-level file read tracker.
//!
//! Implements `brush_core::ExecutionObserver` to track which files have been
//! read during a shell session. When the same file is read again (after context
//! compression discards the agent's memory), the safety filter can return a
//! compressed version instead of dumping the full file again.
//!
//! ## What it tracks
//!
//! - Which commands are file-read commands (cat, head, tail, less, bat, etc.)
//! - Which files have been read and how many times
//! - The content hash of each file at time of read (to detect changes)
//!
//! ## How it communicates with the safety filter
//!
//! The tracker sets environment variables that the Python safety filter reads:
//! - `TSH_READ_COUNT_<hash>`: number of times this file has been read
//! - `TSH_CURRENT_CMD`: the command currently executing (e.g., "cat foo.py")
//!
//! The safety filter uses this to decide compression level:
//! - First read: show structural skeleton (head + structural lines + tail)
//! - Repeat reads: show "[file previously read — N lines, skeleton unchanged]"

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Information about a command being executed.
pub struct CommandInfo {
    /// The command name (e.g., "cat", "/usr/bin/head").
    pub command: String,
    /// Command arguments.
    pub args: Vec<String>,
    /// Working directory when the command was invoked.
    pub working_dir: PathBuf,
}

/// Trait for observing command execution in the shell.
pub trait ExecutionObserver: Send + Sync {
    /// Called when a command starts executing.
    fn on_command_start(&self, info: &CommandInfo);
}

/// Commands that are considered "file readers."
const FILE_READ_COMMANDS: &[&str] = &[
    "cat", "head", "tail", "less", "more", "bat", "batcat", "tac", "nl", "pr", "fold", "fmt",
];

/// Record of a file that has been read during this session.
#[derive(Debug, Clone)]
pub struct FileReadRecord {
    /// Absolute path to the file
    pub path: PathBuf,
    /// Number of times this file has been read
    pub read_count: u32,
    /// Size in bytes at last read
    pub last_size: u64,
    /// Which turn (command index) it was first read
    pub first_read_turn: u32,
    /// Which turn it was last read
    pub last_read_turn: u32,
}

/// Session-level tracker that implements ExecutionObserver.
pub struct SessionTracker {
    inner: Mutex<TrackerState>,
}

struct TrackerState {
    /// Files read during this session, keyed by canonical path
    files: HashMap<PathBuf, FileReadRecord>,
    /// Current command turn counter
    turn: u32,
    /// The file currently being read (if any), for the safety filter to query
    current_read_file: Option<CurrentRead>,
}

/// Info about the file currently being read through the pipe.
#[derive(Debug, Clone)]
pub struct CurrentRead {
    pub path: PathBuf,
    pub read_count: u32,
    pub is_repeat: bool,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TrackerState {
                files: HashMap::new(),
                turn: 0,
                current_read_file: None,
            }),
        }
    }

    /// Returns the current read info (what file is being piped right now).
    /// Called by the router to tell the safety filter how to handle the output.
    pub fn current_read(&self) -> Option<CurrentRead> {
        self.inner.lock().unwrap().current_read_file.clone()
    }

    /// Returns all file read records for the session.
    pub fn all_reads(&self) -> HashMap<PathBuf, FileReadRecord> {
        self.inner.lock().unwrap().files.clone()
    }

    /// Returns how many times a file has been read this session.
    pub fn read_count(&self, path: &PathBuf) -> u32 {
        self.inner
            .lock()
            .unwrap()
            .files
            .get(path)
            .map_or(0, |r| r.read_count)
    }
}

impl ExecutionObserver for SessionTracker {
    fn on_command_start(&self, info: &CommandInfo) {
        let mut state = self.inner.lock().unwrap();
        state.turn += 1;
        let turn = state.turn;

        // Extract just the binary name (strip path)
        let cmd_name = info
            .command
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&info.command);

        // Strip .exe suffix on Windows
        let cmd_name = cmd_name.strip_suffix(".exe").unwrap_or(cmd_name);

        // Check if this is a file-reading command
        if !FILE_READ_COMMANDS.contains(&cmd_name) {
            state.current_read_file = None;
            return;
        }

        // Extract file path(s) from arguments.
        // For cat/head/tail, the file is typically the last arg (or any non-flag arg).
        let file_args: Vec<&String> = info.args.iter().filter(|a| !a.starts_with('-')).collect();

        if file_args.is_empty() {
            state.current_read_file = None;
            return;
        }

        // Track each file argument
        for file_arg in &file_args {
            let path = if PathBuf::from(file_arg).is_absolute() {
                PathBuf::from(file_arg)
            } else {
                info.working_dir.join(file_arg)
            };

            // Get file size (if accessible)
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

            let record = state
                .files
                .entry(path.clone())
                .or_insert_with(|| FileReadRecord {
                    path: path.clone(),
                    read_count: 0,
                    last_size: 0,
                    first_read_turn: turn,
                    last_read_turn: turn,
                });

            record.read_count += 1;
            record.last_read_turn = turn;
            record.last_size = size;

            let is_repeat = record.read_count > 1;

            // Set the current read info for the safety filter
            state.current_read_file = Some(CurrentRead {
                path,
                read_count: record.read_count,
                is_repeat,
            });
        }
    }
}
