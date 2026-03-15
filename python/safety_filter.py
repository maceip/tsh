#!/usr/bin/env python3
"""
safety_filter.py — tsh safety filter (identity pass-through).

This script is spawned by tsh as a long-running subprocess.
It reads text chunks from stdin and writes them to stdout.

Currently this is an identity filter (pass-through). To add
redaction or filtering rules, modify the `filter_chunk` function.
"""

import sys


def filter_chunk(chunk: str) -> str:
    """
    Apply safety filtering / redaction rules to a chunk of text.

    Currently returns the chunk unchanged. To add rules, transform
    the text here before returning it. Examples:
        - Regex-based PII redaction (SSNs, credit card numbers, etc.)
        - Secret/token detection and masking
        - Custom keyword filtering

    Args:
        chunk: A string of text from shell stdout.

    Returns:
        The (possibly redacted) text to send to the terminal.
    """
    # TODO: Add your filtering / redaction rules here.
    return chunk


def main() -> None:
    """Read from stdin line by line, filter, and write to stdout."""
    try:
        for line in sys.stdin:
            filtered = filter_chunk(line)
            sys.stdout.write(filtered)
            sys.stdout.flush()
    except BrokenPipeError:
        # Parent process closed the pipe — exit cleanly.
        pass
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
