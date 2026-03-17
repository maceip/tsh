<div align="center">

# 🪷 tsh

**A shell that manages your agent's context.**

<br>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://img.shields.io/badge/rust-1.77+-7c6ef0?style=for-the-badge&logo=rust&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.77+-7c6ef0?style=for-the-badge&logo=rust&logoColor=white">
</picture>
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://img.shields.io/badge/python-3.10+-4ade80?style=for-the-badge&logo=python&logoColor=white">
  <img alt="Python" src="https://img.shields.io/badge/python-3.10+-4ade80?style=for-the-badge&logo=python&logoColor=white">
</picture>
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://img.shields.io/badge/license-MIT-fbbf24?style=for-the-badge">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-fbbf24?style=for-the-badge">
</picture>

<br><br>

*Full POSIX shell &nbsp;·&nbsp; Smart output limiting &nbsp;·&nbsp; Structural extraction &nbsp;·&nbsp; Local LLM companion*

</div>

<br>

> ```
> $ tsh
> tsh$ cat server.log | grep -i error
> # 3 lines reach the agent. 48,288 don't.
> ```

<br>

tsh is a POSIX-compatible shell built in Rust. It looks and feels like bash — pipes, heredocs, process substitution, job control, all of it. The difference: every byte of stdout flows through a **smart output limiter** that understands code structure. Large outputs are automatically reduced to their structural skeleton — function signatures, class definitions, imports, error lines — so the agent gets the shape of the content without drowning in noise.

No configuration needed. No LLM call. Just pattern-based structural extraction at pipe speed.

<table>
<tr>
<td width="50%">

**🪷 &nbsp; tsh**
```
tsh$ cat large_module.py
# first 100 lines shown...
... [eliding output, showing structure] ...

class DataProcessor:
    def __init__(self, config):
    def process_batch(self, items):
    async def stream_results(self):
class APIClient:
    def authenticate(self):

... [last 20 lines] ...

[tsh: 1,142 lines, 38,491 bytes total.
 Showed first 100 + 12 structural + last 20.
 Elided 1,010 lines.]
```
`~180 lines` in context

</td>
<td width="50%">

**bash**
```
$ cat large_module.py
#!/usr/bin/env python3
"""Module docstring that goes on
for many lines explaining what
this module does..."""
import os
import sys
import json
from pathlib import Path
from typing import Optional, List
  ... 1,132 more lines dumped raw ...
```
`1,142 lines` in context

</td>
</tr>
</table>

<br>

## 🫧 How it works

```
brush-core (fd 1) ──→ pipe ──→ stdout router ──→ smart limiter ──→ agent / terminal
brush-core (fd 2) ──→ pipe ──→ stderr router ──→ terminal (direct)
internal pipes (cmd1|cmd2) ──→ untouched, OS speed
```

The smart limiter (`python/safety_filter.py`) uses **structural pattern matching** — no LLM, no tree-sitter dependency, just fast prefix checks:

<details>
<summary><b>What it keeps vs. what it elides</b></summary>
<br>

**Always shown** — first 100 lines (configurable via `TSH_HEAD_LINES`) and last 20 lines (`TSH_TAIL_LINES`)

**Extracted from the middle** — structurally important lines:

| Language | Patterns kept |
|---|---|
| Python | `def`, `class`, `async def`, `import`, `from` |
| Rust | `pub fn`, `pub struct`, `pub enum`, `impl`, `trait`, `mod`, `use` |
| JavaScript/TS | `function`, `export`, `export default`, `module`, `interface` |
| Java/C# | `public class`, `public static`, `private`, `protected`, `abstract class` |
| C/C++ | `#include`, `#define`, `typedef`, `namespace` |
| Logs | `ERROR`, `WARN`, `FATAL`, `Exception`, `Traceback`, `PANIC` |
| Text | Markdown headings (`#`, `##`, `###`), separators (`===`, `---`) |

**Elided** — everything else in the middle section (access logs, boilerplate, generated code, etc.)

</details>

<details>
<summary><b>Environment variables</b></summary>

| Variable | Default | Purpose |
|---|---|---|
| `TSH_HEAD_LINES` | `100` | Lines to show from the start |
| `TSH_TAIL_LINES` | `20` | Lines to show from the end |
| `TSH_MAX_LINES` | `500` | Hard cap on total output lines |
| `TSH_NO_LIMIT` | `0` | Set to `1` for full pass-through |

</details>

> [!NOTE]
> Internal pipes (`cmd1 | cmd2 | cmd3`) run at OS speed — the limiter only touches the **final stdout** that reaches the terminal/agent.

<br>

## 🪷 Loop memory recovery

The biggest context waste in agent sessions isn't the first read — it's the second, third, and fourth. An agent reads a file, its context compresses, then it re-reads the same file and dumps the entire thing again. tsh tracks this and stops it.

**SessionTracker** (an `ExecutionObserver` on brush-core) watches every file-reading command (`cat`, `head`, `tail`, `less`, `bat`, etc.) and counts reads per file path across the session.

| Read # | What the agent sees | Context cost |
|---|---|---|
| **1st** | Full smart-limited output (head + structure + tail) | ~180 lines |
| **2nd** | `[tsh: repeat read #2 of large_module.py — showing structure only]` + structural lines only | ~50 lines |
| **3rd** | `[tsh: repeat read #3 ...]` + structural lines only | ~50 lines |
| **N-th** | Same pattern, counter increments | ~50 lines |

```
tsh$ cat large_module.py          # loop 1: full smart output (~180 lines)
tsh$ cat large_module.py          # loop 2: structure only (~50 lines)
tsh$ cat large_module.py          # loop 3: structure only (~50 lines)
```

> [!IMPORTANT]
> This happens automatically. No flags, no configuration. The agent doesn't need to know — tsh just stops it from re-consuming context it's already seen.

<br>

## 🪻 Three modes

<table>
<tr><th>Mode</th><th>How</th><th>What happens</th></tr>
<tr>
<td><b>🪷&nbsp;Interactive</b></td>
<td>

```bash
tsh
```
</td>
<td>REPL with <code>tsh$</code> prompt. Full POSIX. All stdout smart-limited.</td>
</tr>
<tr>
<td><b>🫧&nbsp;Command</b></td>
<td>

```bash
tsh -c 'find . -name "*.py" | head -20'
```
</td>
<td>Runs through brush-core, limits output, exits with status.</td>
</tr>
<tr>
<td><b>🪻&nbsp;Script</b></td>
<td>

```bash
echo 'whoami && df -h' | tsh
```
</td>
<td>Reads stdin, handles encoding (UTF-16LE on Windows), runs, limits, exits.</td>
</tr>
</table>

> [!TIP]
> Disable the limiter for debugging: `tsh --no-safety -c 'cat big_file.txt'`

<br>

## 🪷 Quick start

**macOS / Linux / WSL**

```bash
curl -fsSL https://raw.githubusercontent.com/maceip/tsh/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/maceip/tsh/main/install.ps1 | iex
```

Then open a new terminal and run:

```bash
tsh
```

<details>
<summary><b>Build from source</b></summary>

```bash
git clone https://github.com/maceip/tsh && cd tsh
cargo build --release
./target/release/tsh
```
</details>

<details>
<summary><b>Docker</b></summary>

```bash
docker compose up -d
docker compose run --rm tsh
```
</details>

<br>

## 🫧 CLI

```
tsh [OPTIONS]

Options:
  -c, --command <STRING>    Execute command string and exit
      --no-safety           Disable smart limiter (pass-through mode)
  -h, --help                Print help
  -V, --version             Print version
```

<br>

## 🪻 LangExtract — companion extraction engine

tsh ships with [langextract-host](crates/langextract-host/), a zero-copy chunked extraction engine that routes documents through a **local LLM**.

<details>
<summary><b>How it works</b></summary>
<br>

```
┌──────────┐     ┌───────────────┐     ┌──────────────┐     ┌────────────────────┐
│ Document │ ──▸ │  chunk_text() │ ──▸ │ python/shim  │ ──▸ │ AnnotatedDocument  │
│          │     │ 24KB · 1KB    │     │ local model  │     │ class · interval · │
│          │     │ overlap · 0cp │     │ via Ollama   │     │ attributes · align │
└──────────┘     └───────────────┘     └──────────────┘     └────────────────────┘
```

- Splits documents into ~24KB chunks (~6,000 tokens) with 1KB overlap
- Zero-copy chunker operates on `&str` slices — no heap allocations
- Streams each chunk as a JSON line to a long-running Python subprocess
- The Python shim routes to a local model (Ollama, vLLM, any OpenAI-compatible endpoint)
- Input mutations happen before the model sees the text
- Returns structured JSON with character-level alignment intervals

</details>

<details>
<summary><b>Environment variables</b></summary>

| Variable | Default | Purpose |
|---|---|---|
| `OPENAI_API_BASE` | `http://localhost:11434/v1` | Local LLM endpoint |
| `OPENAI_API_KEY` | `local-poc-key` | API key |
| `LLM_MODEL_ID` | `llama3` | Model to use |
| `TSH_MODEL_DIR` | platform cache dir | Model download location |

</details>

> ```bash
> cargo run -p langextract-host
> ```

<br>

## 🪷 Architecture

```
crates/
  tsh/                    Shell binary — pipe routing, smart limiter, brush-core
  langextract-host/       Extraction engine — zero-copy chunker, async streaming
  tsh-model-manager/      Model download, cache, SHA-256 verification
python/
  safety_filter.py        Smart output limiter — structural extraction, head/tail
  shim.py                 LangExtract shim — local model routing, mutations
xtask/                    CI orchestration, 42+ bash pattern tests
```

<br>

## 🫧 Tests

```bash
cargo run -p xtask -- ci
```

<blockquote>

42 bash pattern tests &nbsp;·&nbsp; 50+ Windows compat tests &nbsp;·&nbsp; integration tests against compiled binary

Pipelines &nbsp;·&nbsp; heredocs &nbsp;·&nbsp; process substitution &nbsp;·&nbsp; signal handling &nbsp;·&nbsp; WSL/SSH &nbsp;·&nbsp; JSON/API

Test jungle included: `tests/jungle/` contains large Python, Rust, log, webpack, and JSON files for verifying smart limiting behavior.

</blockquote>

<br>

## 🪻 References

The smart output limiter is informed by recent research on context compression for LLM agents. Full bibliography with open-source links in [`tests/jungle/README.md`](tests/jungle/README.md).

<details>
<summary><b>Key papers</b></summary>
<br>

1. Jha, Erdogan, Kim, Keutzer, Gholami. "Characterizing Prompt Compression Methods for Long Context Inference." *ICML 2024.* Extractive compression achieves up to 10x compression with minimal accuracy loss.

2. Lindenbauer & Slinko. "Simple Observation Masking Is as Efficient as LLM Summarization for Agent Context Management." *NeurIPS DL4Code Workshop, Dec 2025.* Halves cost vs. LLM summarization. [Code](https://github.com/JetBrains-Research/the-complexity-trap)

3. Tree-sitter code skeletonization (Repomix / Aider, 2024–2025). Parse code, return signatures + imports, strip bodies. ~70% token reduction. [Repomix](https://github.com/yamadashy/repomix) · [Aider](https://github.com/Aider-AI/aider)

4. Zhang, Zhao et al. "cAST: AST-Based Code Chunking." *EMNLP 2025 Findings.* [Code](https://github.com/yilinjz/astchunk)

5. Li, Liu, Su, Collier. "Prompt Compression for LLMs: A Survey." *NAACL 2025 (Oral).* [Code](https://github.com/ZongqianLi/Prompt-Compression-Survey)

6. Kang et al. "ACON: Agent Context Optimization." *arXiv, Oct 2025.* 26–54% memory reduction, 95%+ accuracy.

</details>

<br>

<div align="center">

## License

MIT

*Built by [Zunftmax UG](https://zunftmax.com)*

<br>

🪷 🫧 🪻

</div>
