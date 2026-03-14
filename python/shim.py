"""
Python Compliance Shim for LangExtract.

Reads JSON extraction requests from Rust over stdin, applies compliance
mutations (input redaction + output tagging), routes to the local LLM,
and streams results back over stdout.

Mutations:
  - INPUT:  Redacts "CoBro" -> "[REDACTED COMPANY]" before LLM sees the text.
  - OUTPUT: Appends "[COMPLIANCE VERIFIED]" to every extraction_text.
"""

import sys
import json
import os
import langextract as lx

# Configure the environment for a local LLM endpoint (Ollama / vLLM).
os.environ["OPENAI_API_BASE"] = os.environ.get(
    "OPENAI_API_BASE", "http://localhost:11434/v1"
)
os.environ["OPENAI_API_KEY"] = os.environ.get("OPENAI_API_KEY", "local-poc-key")


def process_stream():
    for line in sys.stdin:
        if not line.strip():
            continue

        req = json.loads(line)

        # --- PHASE 1: INPUT MUTATION ---
        # Intercept the text from Rust and scrub it.
        # The LLM will only ever see the redacted version.
        original_text = req.get("text_or_documents", "")
        safe_text = original_text.replace("CoBro", "[REDACTED COMPANY]")

        examples = []
        for ex in req.get("examples", []):
            extractions = [
                lx.data.Extraction(**ext) for ext in ex.get("extractions", [])
            ]
            examples.append(
                lx.data.ExampleData(text=ex["text"], extractions=extractions)
            )

        # Execute the LLM extraction on the sanitized text
        results = lx.extract(
            text_or_documents=safe_text,
            prompt_description=req["prompt_description"],
            examples=examples,
            model_id=os.environ.get("LLM_MODEL_ID", "llama3"),
        )

        # --- PHASE 2: OUTPUT MUTATION ---
        # Modify the data returned by the LLM before Rust sees it.
        output = []
        for doc in results:
            approved_extractions = []
            for ext in doc.extractions:
                modified_extraction_text = (
                    f"{ext.extraction_text} [COMPLIANCE VERIFIED]"
                )

                approved_extractions.append(
                    {
                        "extraction_class": ext.extraction_class,
                        "extraction_text": modified_extraction_text,
                        "char_interval": {
                            "start_pos": ext.char_interval.start_pos,
                            "end_pos": ext.char_interval.end_pos,
                        },
                        "attributes": ext.attributes,
                        "alignment_status": str(ext.alignment_status),
                    }
                )

            output.append(
                {
                    "document_id": doc.document_id,
                    "text": doc.text,
                    "extractions": approved_extractions,
                }
            )

        # Transmit the mutated array back to the Rust host
        sys.stdout.write(json.dumps(output) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    process_stream()
