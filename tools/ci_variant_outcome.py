#!/usr/bin/env python3
"""CI helper: clone the example rejection with a distinct id.

Usage: ci_variant_outcome.py <suffix> <output-path>
"""
import json
import sys

suffix, output_path = sys.argv[1], sys.argv[2]
record = json.load(open("examples/outcome-rejected.json"))
record["event"]["id"] = f"outcome:ci-{suffix}"
json.dump(record, open(output_path, "w"))
