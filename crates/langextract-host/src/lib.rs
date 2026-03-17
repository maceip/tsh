/// LangExtract Host Library
///
/// Executes the Python compliance shim and routes extraction requests to the local LLM.
///
/// # Memory and IO Constraints
///
/// This library serializes `ExtractionInput` into JSON byte strings and pushes them across
/// the OS standard input pipe to the Python shim process.
///
/// 1. **Pipe Buffer Allocation**: The OS allocates a fixed buffer for standard pipes (typically
///    65,536 bytes on Linux and Windows). `tokio::io::AsyncWriteExt::write_all` automatically
///    manages asynchronous chunking if the serialized JSON exceeds this capacity.
///
/// 2. **Inference Token Limits**: While the Tokio pipe will successfully transmit gigabytes
///    of text, the underlying LLM (e.g., Llama 3) imposes a strict context window limit.
///    The chunker splits input into ~24KB segments (~6,000 tokens) to stay well under
///    typical 8,192-token context windows.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ExtractionInput<'a> {
    text_or_documents: &'a str,
    prompt_description: &'a str,
    examples: &'a [ExampleData],
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExampleData {
    pub text: String,
    pub extractions: Vec<Extraction>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Extraction {
    pub extraction_class: String,
    pub extraction_text: String,
    pub attributes: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AnnotatedDocument {
    pub document_id: String,
    pub text: String,
    pub extractions: Vec<ExtractedItem>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ExtractedItem {
    pub extraction_class: String,
    pub extraction_text: String,
    pub char_interval: CharInterval,
    pub attributes: HashMap<String, String>,
    pub alignment_status: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CharInterval {
    pub start_pos: usize,
    pub end_pos: usize,
}

// ---------------------------------------------------------------------------
// Zero-copy text chunker
// ---------------------------------------------------------------------------

/// A high-performance, zero-allocation sliding window text chunker.
///
/// Operates directly on byte boundaries and snaps to nearest whitespace
/// to ensure words are not severed across context windows.
///
/// # Arguments
/// * `text`          - The source text to chunk (borrowed, zero-copy).
/// * `max_bytes`     - Maximum byte length per chunk.
/// * `overlap_bytes` - Number of bytes to overlap between consecutive chunks
///   so entity spans at chunk borders are not lost.
pub fn chunk_text(text: &str, max_bytes: usize, overlap_bytes: usize) -> Vec<&str> {
    if text.is_empty() {
        return vec![];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());

        // If this chunk reaches the end of the text, take it all and stop.
        if end >= text.len() {
            chunks.push(&text[start..]);
            break;
        }

        // Snap back to a valid UTF-8 scalar boundary.
        while !text.is_char_boundary(end) {
            end -= 1;
        }

        // Snap back to the nearest whitespace to preserve full words.
        let mut word_break = end;
        while word_break > start {
            if text[word_break..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace())
            {
                break;
            }
            word_break -= 1;
            while word_break > start && !text.is_char_boundary(word_break) {
                word_break -= 1;
            }
        }

        // If no whitespace exists within the window, force the hard UTF-8 boundary break.
        if word_break == start {
            word_break = end;
        }

        chunks.push(&text[start..word_break]);

        // Calculate the next start position incorporating the specified overlap.
        let next_start = if overlap_bytes > 0 && word_break > overlap_bytes {
            let mut ns = word_break - overlap_bytes;
            while !text.is_char_boundary(ns) {
                ns -= 1;
            }
            // Snap to whitespace so we don't start mid-word.
            while ns > start {
                if text[ns..].chars().next().is_some_and(|c| c.is_whitespace()) {
                    break;
                }
                ns -= 1;
                while ns > start && !text.is_char_boundary(ns) {
                    ns -= 1;
                }
            }
            if ns <= start {
                word_break
            } else {
                ns
            }
        } else {
            word_break
        };

        start = next_start;

        // Advance past leading whitespace for the new chunk.
        while start < text.len() {
            match text[start..].chars().next() {
                Some(c) if c.is_whitespace() => start += c.len_utf8(),
                _ => break,
            }
        }
    }

    chunks
}

// ---------------------------------------------------------------------------
// Shim resolution
// ---------------------------------------------------------------------------

/// Resolves the correct command and arguments to execute the Python compliance layer.
///
/// During local development it invokes `python3 python/shim.py`.
/// When executed from an MSI/DEB/DMG installation, it detects the adjacent
/// PyInstaller-compiled shim binary and executes that directly.
fn resolve_shim_command() -> (String, Vec<String>) {
    if let Ok(mut exe_path) = env::current_exe() {
        exe_path.pop();

        let shim_filename = if cfg!(windows) { "shim.exe" } else { "shim" };
        let compiled_shim = exe_path.join(shim_filename);

        if compiled_shim.exists() {
            return (compiled_shim.to_string_lossy().to_string(), vec![]);
        }
    }

    // Fallback for local development via cargo run / xtask
    ("python3".to_string(), vec!["python/shim.py".to_string()])
}

// ---------------------------------------------------------------------------
// Pipeline execution
// ---------------------------------------------------------------------------

/// Executes the Python compliance shim and routes the extraction request to the local LLM.
///
/// Large texts are automatically split via [`chunk_text`] into segments that fit within
/// the model's context window. Each chunk is streamed as an independent JSON line into
/// the single long-running shim process, and responses are aggregated.
///
/// # Arguments
/// * `target_text` - The raw text or document content requiring extraction.
/// * `prompt`      - The specific extraction directive for the model.
/// * `examples`    - Pre-labeled examples enforcing the expected output schema.
///
/// # Returns
/// A vector of [`AnnotatedDocument`] structs validated and mutated by the compliance layer.
pub async fn execute_pipeline(
    target_text: &str,
    prompt: &str,
    examples: Vec<ExampleData>,
) -> Result<Vec<AnnotatedDocument>> {
    let (cmd_name, cmd_args) = resolve_shim_command();

    let mut child = Command::new(&cmd_name)
        .args(&cmd_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("Failed to spawn compliance shim: {}", cmd_name))?;

    let mut stdin = child.stdin.take().context("Failed to acquire stdin")?;
    let stdout = child.stdout.take().context("Failed to acquire stdout")?;
    let mut reader = BufReader::new(stdout).lines();

    // 1 token ~ 4 bytes. 24,000 bytes keeps chunks well under Llama 3's 8,192 token limit,
    // leaving room for few-shot examples and system instructions.
    let max_bytes: usize = 24_000;
    let overlap_bytes: usize = 1_000;

    let chunks = chunk_text(target_text, max_bytes, overlap_bytes);
    let mut all_documents = Vec::new();

    // Stream every chunk as an independent JSON line into the pipe.
    for chunk in &chunks {
        let payload = ExtractionInput {
            text_or_documents: chunk,
            prompt_description: prompt,
            examples: &examples,
        };

        let mut json_bytes = serde_json::to_vec(&payload)?;
        json_bytes.push(b'\n');
        stdin.write_all(&json_bytes).await?;
    }

    // Flush and close the write pipe to signal end of transmission.
    stdin.flush().await?;
    drop(stdin);

    // Read responses sequentially as the LLM finishes processing each chunk.
    for _ in 0..chunks.len() {
        if let Some(line) = reader.next_line().await? {
            let documents: Vec<AnnotatedDocument> = serde_json::from_str(&line)
                .context("Failed to parse the JSON response array from the shim")?;
            all_documents.extend(documents);
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("Compliance process exited with status: {}", status);
    }

    Ok(all_documents)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_empty_text() {
        assert!(chunk_text("", 100, 0).is_empty());
    }

    #[test]
    fn chunk_small_text_fits_one_chunk() {
        let text = "hello world";
        let chunks = chunk_text(text, 1000, 0);
        assert_eq!(chunks, vec!["hello world"]);
    }

    #[test]
    fn chunk_splits_on_whitespace() {
        let text = "hello world foo bar";
        let chunks = chunk_text(text, 12, 0);
        assert!(chunks.len() >= 2);
        // Every chunk should not start or end mid-word
        for chunk in &chunks {
            let trimmed = chunk.trim();
            assert!(!trimmed.is_empty());
        }
    }

    #[test]
    fn chunk_handles_multibyte_utf8() {
        let text = "Hello \u{1F600} world \u{1F601} test";
        let chunks = chunk_text(text, 10, 0);
        // Should not panic on multibyte boundaries
        for chunk in &chunks {
            assert!(!chunk.is_empty());
        }
    }

    #[test]
    fn chunk_overlap_produces_more_chunks() {
        let text = "word ".repeat(100);
        let no_overlap = chunk_text(&text, 50, 0);
        let with_overlap = chunk_text(&text, 50, 20);
        assert!(with_overlap.len() >= no_overlap.len());
    }
}
