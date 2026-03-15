# Test Jungle

Large files with buried needles for testing tsh's smart output limiter.
Used by `cargo xtask test-jungle` to verify that the limiter preserves
critical information while reducing output volume.

## Needles

### large_python.py (~1140 lines)
- **line 661**: Hardcoded API key `OPENAI_API_KEY = "sk-proj-abc123def456..."` in `ApiClient` class
- **line 802**: `# TODO: CRITICAL` comment on `safe_parse_record()` — silently drops errors and returns None
- **line 901**: `calculate_billing()` has an off-by-one bug: `range(1, billing_period_days)` skips day 0

### large_rust.rs (~930 lines)
- **line 440**: `unsafe` block in `RawBuffer::with_capacity()` with comment `// SAFETY: this is actually unsound, see issue #1234`
- **line 610**: `pub fn process_payment()` chains multiple `.unwrap()` calls with no error handling

### server_log.txt (~2000 lines)
- **line 800**: `ERROR: database connection pool exhausted, 47 pending queries dropped`
- **line 1200**: `FATAL: OOM killer invoked, process nginx (pid 4521) killed`
- **line 1500**: `WARN: SSL certificate expires in 3 days`
- **~10 other WARN entries** sprinkled throughout (slow queries, high memory, consumer lag, etc.)

### config_dump.json (~500 lines)
- `"database_password": "hunter2_production_secret"` in `database.primary`
- `"stripe_secret_key": "sk_live_abc123..."` in `payments`
- Several other secrets: Redis password, Kafka SASL password, OAuth client secrets

### webpack_output.txt (~1500 lines)
- **Near end**: `WARNING: chunk size exceeds 500KB limit`
- **Near end**: `ERROR: circular dependency detected in UserModule`
- Rest is realistic module compilation, optimization, and asset emission noise

---

## Research: Context Compression Techniques

The smart output limiter is informed by the following academic and industry research on
reducing LLM context consumption without losing critical information.

### Tier 1 — Implemented or directly applicable

**Extractive Compression Outperforms Neural Methods (ICML 2024)**
Jha, Erdogan, Kim, Keutzer, Gholami. "Characterizing Prompt Compression Methods for Long Context Inference."
Finding: simple extractive compression (selecting important lines) achieves up to 10x compression
with minimal accuracy loss, outperforming token pruning and neural summarization.
This is what tsh's safety_filter.py implements — structural line extraction.

**The Complexity Trap (NeurIPS DL4Code Workshop, Dec 2025)**
Lindenbauer & Slinko (JetBrains Research / TUM). "Simple Observation Masking Is as Efficient as LLM Summarization for Agent Context Management."
Finding: replacing older tool outputs with placeholders halves cost and matches or exceeds LLM
summarization. LLM summarization actually made agents run 15% longer by masking stopping signals.
Open source: https://github.com/JetBrains-Research/the-complexity-trap

**Tree-Sitter Code Skeletonization (Repomix / Aider, 2024-2025)**
Parse code files with tree-sitter, return function/class signatures + imports, strip bodies.
~70% token reduction. Agent requests specific function bodies on demand.
Open source: https://github.com/yamadashy/repomix, https://github.com/Aider-AI/aider

**Dynamic Context Discovery (Cursor, Jan 2026)**
Write large tool outputs to files instead of inline. Agent uses `tail` to check output ends,
reads more if needed. 46.9% token reduction.

### Tier 2 — Strong evidence, future integration candidates

**cAST: AST-Based Code Chunking (EMNLP 2025 Findings)**
Zhang, Zhao et al. (CMU). Recursively breaks large AST nodes into smaller chunks, merges siblings
while respecting size limits. Self-contained, semantically coherent code units. +4.3 Recall@5.
Open source: https://github.com/yilinjz/astchunk

**Provence: Context Pruning for RAG (ICLR 2025)**
Chirkova et al. (Naver Labs Europe). Given a question and passage, removes sentences not relevant
to the question. Plug-and-play for any LLM. Dynamically detects needed pruning.
Open source: https://github.com/hotchpotch/open_provence

**LLMLingua-2 (ACL 2024)**
Jiang et al. (Microsoft Research). BERT-level encoder trained via data distillation from GPT-4
for token classification. 3x-6x faster than v1, up to 20x compression.
Open source: https://github.com/microsoft/LLMLingua

**ACON: Agent Context Optimization (arXiv, Oct 2025)**
Kang et al. Gradient-free compression guideline optimization in natural language. LLMs analyze
failure causes and update compression guidelines. 26-54% memory reduction, 95%+ accuracy.

**Sculptor: Active Context Management (arXiv, Aug 2025)**
Equips LLMs with context fragmentation, summary/hide/restore, and precise search tools.
LLMs proactively manage their working memory via dynamic context-aware RL.

**Active Context Compression / Focus (arXiv, Jan 2026)**
"Focus" architecture: `start_focus` and `complete_focus` primitives. Agent marks checkpoints,
consolidates learnings into persistent "Knowledge" block while pruning raw history.

### Tier 3 — Longer-term research directions

**500xCompressor (ACL 2025)**
Li et al. Compresses up to 500 tokens into 1 special token via KV-value storage with LoRA adapters.
6x to 480x compression ratios. Requires model-level integration (soft prompts).
Open source: https://github.com/ZongqianLi/500xCompressor

**MemGPT / Letta (2023-2025)**
Virtual context management inspired by OS paging. LLM uses function calls to manage what enters/exits
its context window, retrieving historical data and evicting less relevant data.
Open source: https://github.com/letta-ai/letta

**KV-Distill (arXiv, Mar 2025)**
Chari, Qin, Van Durme. Distills long KV caches into shorter representations. Up to 99% reduction.
Operates at KV-cache level (model internals, not middleware).

**Infini-attention (Google, 2024)**
Compressive memory in attention mechanism. Fixed parameters maintaining compressed representation
of entire context history. 114x compression ratio, handles 1M+ tokens. Architecture-level change.

### Industry tools and practices

**Factory.ai Structured Summarization (Dec 2025)**
Persistent incremental summarization with two thresholds. Key insight: always preserve "breadcrumbs"
(file paths + function names) so the agent can re-retrieve specific sections.

**HumanLayer ACE-FCA (2025)**
Frequent Intentional Compaction + sub-agent isolation. Sub-agents execute noisy operations in
separate context windows, returning only compact summaries. 35k LOC shipped in ~7 hours.
Open source: https://github.com/humanlayer/advanced-context-engineering-for-coding-agents

**OpenCode Dynamic Context Pruning (2025)**
Distill, Compress, Prune tools. Deduplication (removes repeated tool calls) and supersede writes
(removes old reads for subsequently-read files). Session history replaced with placeholders.
Open source: https://github.com/Opencode-DCP/opencode-dynamic-context-pruning

**Context Engineering Toolkit (2025)**
Extractive compression preserving information density. Token-aware truncation. Priority-based assembly.
Open source: https://github.com/jstilb/context-engineering-toolkit

### Key survey

**Prompt Compression for LLMs: A Survey (NAACL 2025, Oral)**
Li, Liu, Su, Collier. Categorizes into hard prompt methods (produce normal text, viable for
middleware) and soft prompt methods (require model integration). Comprehensive taxonomy.
Open source: https://github.com/ZongqianLi/Prompt-Compression-Survey
