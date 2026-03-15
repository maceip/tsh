# Codex Needle-in-a-Haystack Tasks

These tasks are designed to test whether a coding agent (Codex, Claude, etc.)
can find critical information in large files when the output is flowing through
tsh's smart limiter.

## How to run

```bash
# Build tsh first
cargo build -p tsh

# Set your API key
export OPENAI_API_KEY="sk-..."

# Run Codex through tsh (output is limited by the safety filter)
./target/debug/tsh -c 'codex --quiet "Read tests/jungle/large_python.py and find any hardcoded secrets or API keys. Report the exact line number and the secret."'

# Compare: run WITHOUT tsh (raw output, no limiting)
codex --quiet "Read tests/jungle/large_python.py and find any hardcoded secrets or API keys. Report the exact line number and the secret."
```

## Task list

### Task 1: Find the hardcoded API key
**File**: `tests/jungle/large_python.py`
**Prompt**: "Read tests/jungle/large_python.py and find any hardcoded API keys or secrets. Report the exact line number and value."
**Expected**: Should find `OPENAI_API_KEY` at line 661.

### Task 2: Find the critical TODO
**File**: `tests/jungle/large_python.py`
**Prompt**: "Read tests/jungle/large_python.py and find any TODO comments marked as CRITICAL. What's the issue?"
**Expected**: Should find `TODO: CRITICAL` at line 802 about silent error dropping.

### Task 3: Find the unsafe block
**File**: `tests/jungle/large_rust.rs`
**Prompt**: "Read tests/jungle/large_rust.rs and find any unsafe blocks. Are they sound?"
**Expected**: Should find unsound unsafe at line 440.

### Task 4: Find errors in server logs
**File**: `tests/jungle/server_log.txt`
**Prompt**: "Read tests/jungle/server_log.txt and summarize all ERROR and FATAL events."
**Expected**: Should find database pool exhaustion (line 800) and OOM kill (line 1200).

### Task 5: Find secrets in config
**File**: `tests/jungle/config_dump.json`
**Prompt**: "Read tests/jungle/config_dump.json and list all passwords, secrets, and API keys with their JSON paths."
**Expected**: Should find database_password, stripe_secret_key, redis password, kafka password, oauth secrets.

### Task 6: Find build errors
**File**: `tests/jungle/webpack_output.txt`
**Prompt**: "Read tests/jungle/webpack_output.txt and report any errors or warnings."
**Expected**: Should find chunk size warning and circular dependency error.

## Scoring

For each task, record:
- **Found**: Did the agent find the needle? (Y/N)
- **Line accuracy**: Did it report the correct line number? (exact / close / wrong)
- **With tsh**: Was tsh's limiter active? (Y/N)
- **Context used**: How many tokens did the agent consume? (if measurable)

The goal: agents should find needles equally well with or without tsh limiting,
but consume significantly fewer tokens with tsh active.
