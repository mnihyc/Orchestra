#!/usr/bin/env python3
"""Refuse changed search hits or order between two reports of the same fixed corpus."""

import argparse
import json
from pathlib import Path


def complete(row):
    """Only available fused samples establish a baseline for meaning results."""
    return [
        sample
        for sample in row["samples"]
        if not sample["warmup"] and sample.get("fused_ms") is not None
    ]


def signature(sample):
    """Compare final source membership and order, independent of which source arrived first."""
    return sample["ordered_hits"]


def compare(before, after):
    """Return every absent, unavailable or changed case rather than stopping on the first."""
    old = {(row["reader"], row["query"]): row for row in before}
    new = {(row["reader"], row["query"]): row for row in after}
    errors = []
    for key in sorted(old.keys() | new.keys()):
        label = "/".join(key)
        if key not in old or key not in new:
            errors.append(f"{label}: missing case")
            continue
        baseline, actual = complete(old[key]), complete(new[key])
        if not baseline or not actual:
            errors.append(f"{label}: no complete fused sample to compare")
            continue
        expected = signature(baseline[0])
        if not expected:
            errors.append(f"{label}: no ordered hit identities")
            continue
        if any(signature(sample) != expected for sample in baseline):
            errors.append(f"{label}: baseline results are not stable")
        if any(signature(sample) != expected for sample in actual):
            errors.append(f"{label}: changed hits, source membership or order")
        old_agent = next((sample["agent_payloads"] for sample in baseline
                          if sample.get("search_shared_fused_ms") is not None), None)
        if old_agent is not None and any(
            sample.get("search_shared_fused_ms") is not None
            and sample.get("agent_payloads") != old_agent for sample in actual
        ):
            errors.append(f"{label}: changed agent hits or order")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    args = parser.parse_args()
    before, after = (json.loads(path.read_text()) for path in (args.before, args.after))
    errors = compare(before, after)
    if errors:
        for error in errors:
            print(error)
        return 1
    print(f"Unchanged ordered results for {len(before)} reader/query cases")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
