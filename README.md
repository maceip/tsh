# 🪷 tsh

**A shell that manages your agent's context.**

```
$ tsh
tsh$ cat server.log | grep -i error
# 3 lines reach the agent. 48,288 don't.
```

tsh is a POSIX-compatible shell built in Rust. It looks and feels like bash — pipes, heredocs, process substitution, job control, all of it. The difference: every byte of stdout flows through a routing layer that filters, summarizes, and manages what actually reaches the agent's context window or memory. No more blowing your token budget on 50,000 lines of logs when you needed three.

Powered by [brush-core](https://github.com/reubeno/brush). Filtered by a pluggable Python safety layer. Extractions powered by a local model via [LangExtract](crates/langextract-host/).

---

## 🫧 How it works

```
brush-core (fd 1) ──→ pipe ──→ stdout router ──→ safety filter ──→ agent / terminal
brush-core (fd 2) ──→ pipe ──→ stderr router ──→ terminal (direct)
internal pipes (cmd1|cmd2) ──→ untouched, OS speed
```

1. Commands run inside brush-core (a Rust implementation of bash)
2. Their stdout is captured via injected file descriptors — not the terminal
3. A Tokio async router reads the pipe and forwards text to a long-running Python safety process
4. The safety layer filters output — managing what reaches the agent's context or memory
5. Clean output reaches the terminal / agent
6. Binary data is detected (null bytes) and bypasses the filter automatically
7. stderr always goes directly to terminal — no filtering
8. Internal pipes between commands (`grep foo | sort`) run at OS speed, untouched by the router

## 🪻 Three modes

**Interactive** — drop in like bash.
```
$ tsh
tsh$ ls -la
tsh$ cat server.log | grep -i error   # only relevant lines reach the agent
tsh$ exit
```

**Command string** — run and exit, like `bash -c`.
```bash
tsh -c 'find . -name "*.py" | head -20'
```

**Piped script** — pipe a script in via stdin.
```bash
echo 'whoami && df -h' | tsh
```

Disable the safety layer for debugging:
```bash
tsh --no-safety -c 'echo "raw, unfiltered output"'
```

## 🪷 Get running

```bash
git clone https://github.com/user/tsh && cd tsh
cargo build --release

# Use it
./target/release/tsh
./target/release/tsh -c 'echo "hello from tsh"'
```

## 🫧 CLI reference

```
tsh [OPTIONS]

Options:
  -c, --command <STRING>    Execute command string and exit
      --no-safety           Disable safety filter (pass-through mode)
  -h, --help                Print help
  -V, --version             Print version
```

| Condition | Mode | Behavior |
|---|---|---|
| `-c "cmd"` provided | Command | Runs string through brush-core, stdout filtered, exits |
| stdin is a pipe | Script | Reads stdin as script, runs through brush-core, exits |
| stdin is a terminal | Interactive | REPL with `tsh$` prompt, stdout filtered live |

## 🪻 LangExtract — companion extraction library

tsh ships with [langextract-host](crates/langextract-host/), a zero-copy chunked extraction engine that routes documents through a **local LLM** via LangExtract.

**What it does:**
- Splits documents into ~24KB chunks (~6,000 tokens) with 1KB overlap
- Streams each chunk as a JSON line to a Python subprocess
- The Python shim (`python/shim.py`) routes to a local model (Ollama, vLLM, any OpenAI-compatible endpoint)
- Input mutations happen before the model sees the text
- Output mutations happen before results return
- Returns structured `AnnotatedDocument` JSON with character-level alignment

**Run it directly:**
```bash
cargo run -p langextract-host
```

**Environment variables:**

| Variable | Default | Purpose |
|---|---|---|
| `OPENAI_API_BASE` | `http://localhost:11434/v1` | Local LLM endpoint |
| `OPENAI_API_KEY` | `local-poc-key` | API key |
| `LLM_MODEL_ID` | `llama3` | Model to use |
| `TSH_MODEL_DIR` | platform cache dir | Model download location |

## 🪷 Architecture

```
crates/
  tsh/                    Shell binary — pipe routing, safety layer, brush-core
  langextract-host/       Extraction engine — zero-copy chunker, async streaming
  tsh-model-manager/      Model download, cache, SHA-256 verification
python/
  shim.py                 LangExtract shim — local model routing, mutations
  safety_filter.py        Output safety filter (pluggable)
xtask/                    CI orchestration, 42+ bash pattern tests
```

## 🫧 Tests

```bash
cargo run -p xtask -- ci
```

42 bash pattern tests (pipelines, heredocs, process substitution, signal handling, WSL/SSH), 50+ Windows compatibility tests, and integration tests. Windows CI skips POSIX-only tests automatically.

## License

MIT

*Built by [Zunftmax UG](https://zunftmax.com)*
