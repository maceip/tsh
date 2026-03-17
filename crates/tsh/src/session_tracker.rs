//! Session-level file read tracker.
//!
//! Tracks which files have been read during a shell session so the
//! safety filter can compress repeat reads instead of dumping the
//! full file again.

use std::path::PathBuf;
use std::sync::Mutex;

/// Info about the file currently being read through the pipe.
#[derive(Debug, Clone)]
pub struct CurrentRead {
    pub path: PathBuf,
    pub read_count: u32,
    pub is_repeat: bool,
}

/// Session-level tracker for file reads.
pub struct SessionTracker {
    inner: Mutex<Option<CurrentRead>>,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// Returns the current read info (what file is being piped right now).
    /// Called by the router to tell the safety filter how to handle the output.
    pub fn current_read(&self) -> Option<CurrentRead> {
        self.inner.lock().unwrap().clone()
    }
}
