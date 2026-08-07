#!/usr/bin/env python3
"""CI check: published prices survive the micros conversion exactly.

Reads `owi prices --dry-run` JSON on stdin. $5.00/Mtok input must become
5_000_000 micros, not 4_999_999 — a one-micro drift here is a rounding bug in
a money path. Also asserts no offering identifier repeats a provider segment.
"""
import json
import sys

result = json.load(sys.stdin)
offerings = {o["id"]: o for o in result["offerings"]}

EXPECTED = {
    "offering:anthropic/claude-sonnet-5": (2_000_000, 10_000_000),
    "offering:anthropic/claude-opus-4-5": (5_000_000, 25_000_000),
    "offering:openai/gpt-5-mini": (250_000, 2_000_000),
    "offering:deepseek/deepseek-chat": (280_000, 420_000),
}
for offering_id, (want_in, want_out) in EXPECTED.items():
    got = offerings[offering_id]
    assert got["input_micros_per_million_tokens"] == want_in, (offering_id, got)
    assert got["output_micros_per_million_tokens"] == want_out, (offering_id, got)

assert not any(
    f"/{provider}/" in offering_id
    for offering_id in offerings
    for provider in ("gemini", "xai", "deepseek", "mistral")
), "an offering identifier repeats its provider segment"

print(f"price adapter verified: {len(offerings)} offerings")
