#!/usr/bin/env python3
"""
safety_filter.py — tsh smart output limiter.

Zero dependencies (stdlib only). Spawned by tsh as a long-running subprocess.

Behavior:
  - Short output (≤ HEAD_LINES): pass through unchanged.
  - Large output (> HEAD_LINES): show first HEAD_LINES, then extract only
    structural/important lines (function defs, class defs, imports, headings,
    errors), then show last TAIL_LINES, then a footer with totals.

This prevents multi-megabyte dumps from flooding the terminal or consuming
an LLM agent's entire context window, while preserving the information
that matters: the structure of the content and the beginning/end.

Configurable via environment variables:
  TSH_HEAD_LINES   - lines to show from the start (default: 100)
  TSH_TAIL_LINES   - lines to show from the end (default: 20)
  TSH_MAX_LINES    - hard cap on total output lines (default: 500)
  TSH_NO_LIMIT     - set to "1" to disable limiting (full pass-through)
"""

import os
import sys

HEAD_LINES = int(os.environ.get("TSH_HEAD_LINES", "100"))
TAIL_LINES = int(os.environ.get("TSH_TAIL_LINES", "20"))
MAX_OUTPUT_LINES = int(os.environ.get("TSH_MAX_LINES", "500"))
NO_LIMIT = os.environ.get("TSH_NO_LIMIT") == "1"

# Patterns that indicate a "structural" line worth keeping even when
# we're eliding the middle of a large output. These are simple
# startswith checks — no regex, no dependencies.
CODE_STRUCTURAL_PREFIXES = (
    "def ",
    "class ",
    "import ",
    "from ",
    "async def ",
    "    def ",
    "    async def ",
    "    class ",
    "pub fn ",
    "pub struct ",
    "pub enum ",
    "pub trait ",
    "pub mod ",
    "pub type ",
    "pub const ",
    "pub async fn ",
    "fn ",
    "struct ",
    "enum ",
    "impl ",
    "trait ",
    "mod ",
    "use ",
    "const ",
    "static ",
    "type ",
    "function ",
    "export ",
    "export default ",
    "module ",
    "package ",
    "interface ",
    "abstract class ",
    "public class ",
    "public static ",
    "private ",
    "protected ",
    "#include ",
    "#define ",
    "typedef ",
    "namespace ",
)

# Patterns for text/log structural lines
TEXT_STRUCTURAL_PREFIXES = (
    "# ",
    "## ",
    "### ",
    "ERROR",
    "WARN",
    "FATAL",
    "Exception",
    "Traceback",
    "===",
    "---",
    "***",
)

# Patterns that can appear anywhere in a line (not just at start)
LOG_LEVEL_MARKERS = (
    "[ERROR]",
    "[FATAL]",
    "[WARN]",
    "[WARNING]",
    "[CRITICAL]",
    " ERROR ",
    " FATAL ",
    " PANIC ",
)


def is_structural(line: str) -> bool:
    """Check if a line is structurally important (worth keeping when eliding)."""
    stripped = line.lstrip()
    if not stripped:
        return False
    for prefix in CODE_STRUCTURAL_PREFIXES:
        if stripped.startswith(prefix):
            return True
    for prefix in TEXT_STRUCTURAL_PREFIXES:
        if stripped.startswith(prefix):
            return True
    # Log lines with level markers anywhere in the line
    for marker in LOG_LEVEL_MARKERS:
        if marker in line:
            return True
    return False


def main() -> None:
    if NO_LIMIT:
        # Full pass-through, no limiting
        try:
            for line in sys.stdin:
                sys.stdout.write(line)
                sys.stdout.flush()
        except (BrokenPipeError, KeyboardInterrupt):
            pass
        return

    output_lines = 0        # lines written to stdout
    total_lines = 0         # total lines read from stdin
    total_bytes = 0         # total bytes read
    elided_lines = 0        # lines we skipped
    structural_lines = 0    # structural lines extracted from the elided section
    in_elision = False      # are we currently eliding?
    tail_buffer = []        # ring buffer for tail lines
    elision_announced = False
    repeat_read = False     # is this a repeat read of the same file?
    repeat_path = ""        # path of the file being re-read
    repeat_count = 0        # how many times it's been read

    try:
        for line in sys.stdin:
            # Command boundary reset — flush state for new command
            if line.startswith("[tsh:new-command]"):
                # Emit footer for previous command if we were eliding
                if repeat_read:
                    sys.stdout.write(
                        f"\n[tsh: repeat read #{repeat_count} — {total_lines} lines, "
                        f"{structural_lines} structural shown, "
                        f"{elided_lines} elided.]\n"
                    )
                    sys.stdout.flush()
                elif in_elision:
                    sys.stdout.write(f"\n... [last {len(tail_buffer)} lines] ...\n\n")
                    for tl in tail_buffer:
                        sys.stdout.write(tl)
                    sys.stdout.write(
                        f"\n[tsh: {total_lines} lines, {total_bytes:,} bytes total. "
                        f"Showed first {HEAD_LINES} + {structural_lines} structural + "
                        f"last {len(tail_buffer)}. "
                        f"Elided {elided_lines} lines.]\n"
                    )
                    sys.stdout.flush()

                # Reset all state for next command
                output_lines = 0
                total_lines = 0
                total_bytes = 0
                elided_lines = 0
                structural_lines = 0
                in_elision = False
                tail_buffer = []
                elision_announced = False
                repeat_read = False
                repeat_path = ""
                repeat_count = 0
                continue

            # Check for tsh repeat-read metadata header
            if total_lines == 0 and line.startswith("[tsh:repeat-read"):
                # Parse: [tsh:repeat-read path=/foo/bar count=3]
                repeat_read = True
                for part in line.strip().strip("[]").split():
                    if part.startswith("path="):
                        repeat_path = part[5:]
                    elif part.startswith("count="):
                        repeat_count = int(part[6:])
                # On repeat reads, show ONLY structural lines + a note
                sys.stdout.write(
                    f"[tsh: repeat read #{repeat_count} of {repeat_path} — "
                    f"showing structure only]\n\n"
                )
                sys.stdout.flush()
                output_lines += 2
                continue

            total_lines += 1
            total_bytes += len(line.encode("utf-8", errors="replace"))

            # Repeat reads: structural-only mode from the start
            if repeat_read:
                if is_structural(line):
                    sys.stdout.write(line)
                    sys.stdout.flush()
                    output_lines += 1
                    structural_lines += 1
                else:
                    elided_lines += 1
                continue

            # Phase 1: pass through the first HEAD_LINES unchanged
            if total_lines <= HEAD_LINES:
                sys.stdout.write(line)
                sys.stdout.flush()
                output_lines += 1
                continue

            # Phase 2: we're past HEAD_LINES — switch to smart extraction
            if not in_elision:
                in_elision = True

            # Always capture tail lines (ring buffer)
            tail_buffer.append(line)
            if len(tail_buffer) > TAIL_LINES:
                tail_buffer.pop(0)

            # Extract structural lines (but respect MAX_OUTPUT_LINES)
            if is_structural(line) and output_lines < MAX_OUTPUT_LINES:
                if not elision_announced:
                    sys.stdout.write(f"\n... [eliding output, showing structure] ...\n\n")
                    sys.stdout.flush()
                    elision_announced = True
                    output_lines += 2
                sys.stdout.write(line)
                sys.stdout.flush()
                output_lines += 1
                structural_lines += 1
            else:
                elided_lines += 1

    except (BrokenPipeError, KeyboardInterrupt):
        pass

    # Phase 3: footer
    if repeat_read:
        try:
            sys.stdout.write(
                f"\n[tsh: repeat read #{repeat_count} — {total_lines} lines, "
                f"{structural_lines} structural shown, "
                f"{elided_lines} elided.]\n"
            )
            sys.stdout.flush()
        except BrokenPipeError:
            pass
    elif in_elision:
        try:
            sys.stdout.write(f"\n... [last {len(tail_buffer)} lines] ...\n\n")
            for tl in tail_buffer:
                sys.stdout.write(tl)
            sys.stdout.flush()

            sys.stdout.write(
                f"\n[tsh: {total_lines} lines, {total_bytes:,} bytes total. "
                f"Showed first {HEAD_LINES} + {structural_lines} structural + "
                f"last {len(tail_buffer)}. "
                f"Elided {elided_lines} lines.]\n"
            )
            sys.stdout.flush()
        except BrokenPipeError:
            pass


if __name__ == "__main__":
    main()
