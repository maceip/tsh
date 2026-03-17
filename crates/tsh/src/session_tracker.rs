//! Session-level file read tracker.
//!
//! Tracks file-reader commands across a tsh shell session so the safety filter
//! can collapse repeat reads into structure-only summaries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FILE_READ_COMMANDS: &[&str] = &[
    "cat", "head", "tail", "less", "more", "bat", "batcat", "tac", "nl", "pr", "fold", "fmt",
];

#[derive(Debug, Clone)]
pub struct CurrentRead {
    pub path: PathBuf,
    pub read_count: u32,
    pub is_repeat: bool,
}

#[derive(Debug, Clone, Default)]
pub struct CommandSnapshot {
    pub turn: u32,
    pub current_read: Option<CurrentRead>,
}

pub struct SessionTracker {
    inner: Mutex<TrackerState>,
}

#[derive(Default)]
struct TrackerState {
    files: HashMap<PathBuf, u32>,
    turn: u32,
    current_read: Option<CurrentRead>,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(TrackerState::default()),
        }
    }

    pub fn current_snapshot(&self) -> CommandSnapshot {
        let state = self.inner.lock().unwrap();
        CommandSnapshot {
            turn: state.turn,
            current_read: state.current_read.clone(),
        }
    }

    /// Records the next command that is about to execute.
    pub fn on_command_start(&self, command: &str, working_dir: &Path) {
        let mut state = self.inner.lock().unwrap();
        state.turn += 1;

        let path = detect_file_read_path(command, working_dir);
        state.current_read = path.map(|path| {
            let read_count = {
                let entry = state.files.entry(path.clone()).or_insert(0);
                *entry += 1;
                *entry
            };

            CurrentRead {
                path,
                read_count,
                is_repeat: read_count > 1,
            }
        });
    }
}

pub fn command_reads_file(command: &str, working_dir: &Path) -> bool {
    detect_file_read_path(command, working_dir).is_some()
}

fn detect_file_read_path(command: &str, working_dir: &Path) -> Option<PathBuf> {
    let tokens = shell_split(command);
    let command_token_index = tokens.iter().position(|token| !is_env_assignment(token))?;
    let command_name = normalize_command_name(&tokens[command_token_index]);

    if !FILE_READ_COMMANDS.contains(&command_name.as_str()) {
        return None;
    }

    let path_token = tokens[command_token_index + 1..]
        .iter()
        .find(|token| !token.starts_with('-'))?;

    let path = PathBuf::from(path_token);
    Some(if path.is_absolute() {
        path
    } else {
        working_dir.join(path)
    })
}

fn is_env_assignment(token: &str) -> bool {
    token
        .split_once('=')
        .is_some_and(|(name, _)| !name.is_empty() && !name.contains(['/', '\\']))
}

fn normalize_command_name(token: &str) -> String {
    Path::new(token)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(token)
        .trim_end_matches(".exe")
        .to_string()
}

fn shell_split(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut single_quote = false;
    let mut double_quote = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !double_quote => {
                single_quote = !single_quote;
            }
            '"' if !single_quote => {
                double_quote = !double_quote;
            }
            '\\' if !single_quote => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            c if c.is_whitespace() && !single_quote && !double_quote => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}
