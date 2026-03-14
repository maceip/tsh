use anyhow::{Context, Result};
use std::env;
use std::path::PathBuf;
use xshell::{cmd, Shell};

#[cfg(target_os = "windows")]
fn run_ci_tests(sh: &Shell) -> Result<()> {
    println!("Skipping POSIX integration tests on Windows host.");
    cmd!(sh, "cargo test --workspace --lib").run()?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn run_ci_tests(sh: &Shell) -> Result<()> {
    cmd!(sh, "cargo test --workspace --all-features").run()?;
    Ok(())
}

fn print_usage() {
    eprintln!("Usage: cargo run -p xtask -- <command>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  run-host       Run the langextract-host binary directly");
    eprintln!("  run-tsh        Run the tsh (Token Shell) binary");
    eprintln!("  ci             Run the CI test suite (platform-aware)");
    eprintln!("  fetch-model    Download the default LLM model to the cache");
    eprintln!("  list-models    List cached model files");
    eprintln!("  clean-models   Remove all cached model files");
}

fn main() -> Result<()> {
    let sh = Shell::new()?;
    let task = env::args().nth(1);

    let manifest_dir =
        env::var("CARGO_MANIFEST_DIR").context("CARGO_MANIFEST_DIR is not set")?;
    let workspace_root = PathBuf::from(manifest_dir)
        .parent()
        .context("xtask directory has no parent")?
        .to_path_buf();

    let python_dir = workspace_root.join("python");
    // SAFETY: single-threaded xtask binary
    unsafe {
        env::set_var("PYTHONPATH", &python_dir);
    }

    match task.as_deref() {
        Some("run-host") => {
            cmd!(sh, "cargo run -p langextract-host").run()?;
        }
        Some("run-tsh") => {
            cmd!(sh, "cargo run -p tsh").run()?;
        }
        Some("ci") => {
            run_ci_tests(&sh)?;
        }
        Some("fetch-model") => {
            let model_name = env::args().nth(2).unwrap_or_else(|| "gemma3-1b-q4".to_string());
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(fetch_model(&model_name))?;
        }
        Some("list-models") => {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(list_models())?;
        }
        Some("clean-models") => {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(clean_models())?;
        }
        _ => {
            print_usage();
            std::process::exit(1);
        }
    }
    Ok(())
}

async fn fetch_model(model_name: &str) -> Result<()> {
    let registry = tsh_model_manager::default_registry();
    let entry = registry
        .get(model_name)
        .with_context(|| {
            let available: Vec<&str> = registry.keys().map(|k| k.as_str()).collect();
            format!(
                "Unknown model '{}'. Available: {}",
                model_name,
                available.join(", ")
            )
        })?;

    let path = tsh_model_manager::ensure_model(entry, Some(|downloaded: u64, total: u64| {
        if total > 0 {
            let pct = (downloaded as f64 / total as f64) * 100.0;
            let mb = downloaded as f64 / 1_048_576.0;
            let total_mb = total as f64 / 1_048_576.0;
            eprint!("\r  {:.1} / {:.1} MB ({:.0}%)", mb, total_mb, pct);
        } else {
            let mb = downloaded as f64 / 1_048_576.0;
            eprint!("\r  {:.1} MB downloaded", mb);
        }
    })).await?;

    eprintln!();
    println!("Model ready at: {}", path.display());
    Ok(())
}

async fn list_models() -> Result<()> {
    let models = tsh_model_manager::list_cached_models().await?;
    if models.is_empty() {
        println!("No cached models. Run: cargo xtask fetch-model");
        return Ok(());
    }

    let cache_dir = tsh_model_manager::model_cache_dir()?;
    println!("Cache directory: {}", cache_dir.display());
    println!();

    for (name, size) in &models {
        let mb = *size as f64 / 1_048_576.0;
        println!("  {:<50} {:>8.1} MB", name, mb);
    }

    let total: u64 = models.iter().map(|(_, s)| s).sum();
    println!();
    println!("  Total: {:.1} MB", total as f64 / 1_048_576.0);
    Ok(())
}

async fn clean_models() -> Result<()> {
    let models = tsh_model_manager::list_cached_models().await?;
    if models.is_empty() {
        println!("No cached models to clean.");
        return Ok(());
    }

    for (name, _) in &models {
        tsh_model_manager::remove_cached_model(name).await?;
        println!("Removed: {}", name);
    }

    println!("All cached models removed.");
    Ok(())
}
