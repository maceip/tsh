use anyhow::{Context, Result};
use clap::Parser;
use langextract_host::AnnotatedDocument;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::{self, IsTerminal, Read};
use tokio::process::Command;

/// Token Shell (tsh) - LangExtract Compliance Pipeline
#[derive(Parser, Debug)]
#[command(author, version, about = "Token Shell for LangExtract Compliance Pipeline")]
struct Args {
    /// The specific prompt description for the model
    #[arg(short, long)]
    prompt: Option<String>,

    /// Optional: Path to a JSON file containing few-shot ExampleData
    #[arg(short, long)]
    examples: Option<String>,

    /// Output the results as a unified JSON array for piping into jq, etc.
    #[arg(long, default_value_t = false)]
    json: bool,
}

// ---------------------------------------------------------------------------
// PowerShell / encoding middleware
// ---------------------------------------------------------------------------

/// Decodes raw stdin bytes into a UTF-8 String, handling the PowerShell 5.1
/// UTF-16LE encoding hazard on Windows.
///
/// Detection order:
/// 1. UTF-16LE BOM (`FF FE`) → transcode via `encoding_rs`
/// 2. UTF-8 BOM (`EF BB BF`) → strip BOM, validate UTF-8
/// 3. Valid UTF-8 → use directly
/// 4. (Windows only) Fallback → attempt UTF-16LE without BOM
#[cfg(windows)]
fn decode_stdin_bytes(raw: Vec<u8>) -> Result<String> {
    // UTF-16LE BOM
    if raw.len() >= 2 && raw[0] == 0xFF && raw[1] == 0xFE {
        let (decoded, _encoding, had_errors) = encoding_rs::UTF_16LE.decode(&raw[2..]);
        if had_errors {
            anyhow::bail!(
                "Failed to decode UTF-16LE input from PowerShell. \
                 Ensure $OutputEncoding is set to UTF-8."
            );
        }
        return Ok(decoded.into_owned());
    }

    // UTF-8 BOM
    if raw.len() >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF {
        return String::from_utf8(raw[3..].to_vec())
            .context("Input contains invalid UTF-8 after BOM");
    }

    // Try UTF-8 first
    if let Ok(s) = String::from_utf8(raw.clone()) {
        return Ok(s);
    }

    // Fallback: try UTF-16LE without BOM (PowerShell 5.1 sometimes omits it)
    let (decoded, _encoding, had_errors) = encoding_rs::UTF_16LE.decode(&raw);
    if had_errors {
        anyhow::bail!(
            "Stdin is not valid UTF-8 or UTF-16LE. \
             Run: $OutputEncoding = [System.Text.Encoding]::UTF8"
        );
    }
    Ok(decoded.into_owned())
}

#[cfg(not(windows))]
fn decode_stdin_bytes(raw: Vec<u8>) -> Result<String> {
    // Strip UTF-8 BOM if present
    if raw.len() >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF {
        return String::from_utf8(raw[3..].to_vec())
            .context("Input contains invalid UTF-8 after BOM");
    }
    String::from_utf8(raw).context("Stdin input is not valid UTF-8")
}

// ---------------------------------------------------------------------------
// Output formatting
// ---------------------------------------------------------------------------

/// Routes the output to stdout based on the requested format.
fn handle_output(documents: &[AnnotatedDocument], output_json: bool) {
    if output_json {
        match serde_json::to_string_pretty(&documents) {
            Ok(json_str) => println!("{}", json_str),
            Err(e) => eprintln!("Failed to serialize documents to JSON: {}", e),
        }
    } else {
        println!(
            "Extraction successful. Received {} document(s).",
            documents.len()
        );
        for doc in documents {
            println!("Document ID: {}", doc.document_id);
            for ext in &doc.extractions {
                println!(
                    "  [{}] {}: '{}'",
                    ext.alignment_status, ext.extraction_class, ext.extraction_text
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    // Mode 1: Standard Unix / PowerShell Utility (Piped Input)
    if !io::stdin().is_terminal() {
        let mut raw_bytes = Vec::new();
        io::stdin()
            .read_to_end(&mut raw_bytes)
            .context("Failed to read raw bytes from stdin")?;

        let buffer = decode_stdin_bytes(raw_bytes)?;

        if buffer.trim().is_empty() {
            anyhow::bail!("No input provided via stdin.");
        }

        let args = Args::parse();
        let prompt = args
            .prompt
            .unwrap_or_else(|| "Extract all relevant entities.".to_string());

        let mut parsed_examples = Vec::new();
        if let Some(examples_path) = &args.examples {
            let examples_raw = tokio::fs::read_to_string(examples_path)
                .await
                .with_context(|| format!("Failed to read examples file: {}", examples_path))?;
            parsed_examples = serde_json::from_str(&examples_raw)
                .context("Failed to parse examples JSON")?;
        }

        println!(
            "Executing piped extraction. Target size: {} bytes",
            buffer.len()
        );

        match langextract_host::execute_pipeline(&buffer, &prompt, parsed_examples).await {
            Ok(documents) => handle_output(&documents, args.json),
            Err(e) => {
                eprintln!("Pipeline execution failed: {:?}", e);
                std::process::exit(1);
            }
        }

        return Ok(());
    }

    // Mode 2: Interactive Token Shell
    let mut rl = DefaultEditor::new()?;
    println!("Token Shell (tsh) initialized. Type 'help' for commands, 'exit' to quit.");

    loop {
        let readline = rl.readline("tsh$ ");
        match readline {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let _ = rl.add_history_entry(line);

                let args = match shell_words::split(line) {
                    Ok(a) => a,
                    Err(e) => {
                        eprintln!("Parse error: {}", e);
                        continue;
                    }
                };
                if args.is_empty() {
                    continue;
                }

                match args[0].as_str() {
                    "exit" | "quit" => break,
                    "help" => {
                        println!("Token Shell (tsh) built-in commands:");
                        println!("  extract <prompt> <text>         - Run extraction pipeline");
                        println!("  extract-json <prompt> <text>    - Extract and output JSON");
                        println!("  help                            - Show this help");
                        println!("  exit / quit                     - Exit the shell");
                        println!("  <any other command>             - Passed to system shell");
                    }
                    "extract" | "extract-json" => {
                        if args.len() < 3 {
                            eprintln!("Usage: extract <prompt> <target_text>");
                            continue;
                        }
                        let prompt = &args[1];
                        let target_text = &args[2];
                        let as_json = args[0] == "extract-json";

                        println!("Executing extraction...");
                        match langextract_host::execute_pipeline(target_text, prompt, vec![]).await
                        {
                            Ok(documents) => handle_output(&documents, as_json),
                            Err(e) => eprintln!("Extraction failed: {}", e),
                        }
                    }
                    cmd => {
                        let child = Command::new(cmd).args(&args[1..]).spawn();
                        match child {
                            Ok(mut process) => {
                                let _ = process.wait().await;
                            }
                            Err(e) => {
                                eprintln!("{}: command not found or error: {}", cmd, e);
                            }
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => break,
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        }
    }

    Ok(())
}
