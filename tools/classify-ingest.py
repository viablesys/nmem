#!/usr/bin/env python3
"""
Ingest agent-labeled JSON into corpus format for training.

Takes the agent's classification output and merges with extracted text
to produce a balanced training corpus.

Agent labeling workflow:
1. Extract: python3 tools/classify-extract.py --limit 500 --output /tmp/unlabeled.json
2. Agent reads /tmp/unlabeled.json in batches of ~25, classifies each as think/act
3. Agent writes labels: [{"id": 123, "type": "think"}, ...]
4. Ingest: python3 tools/classify-ingest.py --extracted /tmp/unlabeled.json --labels /tmp/labels.json --output tools/corpus-think-act-500.json
5. Train: python3 tools/classify-train.py --corpus tools/corpus-think-act-500.json --output models/think-act.json

Usage:
    python3 tools/classify-ingest.py --extracted /tmp/unlabeled.json --labels /tmp/labels.json --output tools/corpus-think-act-500.json
    python3 tools/classify-ingest.py --extracted /tmp/unlabeled.json --labels /tmp/labels.json --no-balance
"""

import argparse
import json
import random
import sys


def load_json(path):
    with open(path) as f:
        return json.load(f)


def merge(extracted, labels):
    """Merge extracted prompts with agent labels by ID."""
    # Index extracted by ID
    by_id = {e["id"]: e for e in extracted}

    corpus = []
    missing = 0
    invalid = 0

    for label_entry in labels:
        pid = label_entry["id"]
        label = label_entry.get("type")

        if label not in ("think", "act"):
            invalid += 1
            continue

        if pid not in by_id:
            missing += 1
            continue

        ext = by_id[pid]
        prefix = "[agent] " if ext["source"] == "agent" else "[user] "

        corpus.append({
            "id": pid,
            "text": prefix + ext["text"],
            "type": label,
        })

    if missing:
        print(f"Warning: {missing} label IDs not found in extracted data", file=sys.stderr)
    if invalid:
        print(f"Warning: {invalid} labels with invalid type (not think/act)", file=sys.stderr)

    return corpus


def balance_corpus(corpus):
    """Balance to equal think/act counts."""
    think = [r for r in corpus if r["type"] == "think"]
    act = [r for r in corpus if r["type"] == "act"]

    min_count = min(len(think), len(act))
    if min_count == 0:
        print("Warning: one class is empty, cannot balance", file=sys.stderr)
        return corpus

    random.shuffle(think)
    random.shuffle(act)
    balanced = think[:min_count] + act[:min_count]
    random.shuffle(balanced)

    print(f"Balanced: {min_count} think + {min_count} act = {len(balanced)} total")
    return balanced


def main():
    parser = argparse.ArgumentParser(
        description="Ingest agent-labeled JSON into training corpus"
    )
    parser.add_argument("--extracted", required=True, help="Extracted prompts JSON (from classify-extract.py)")
    parser.add_argument("--labels", required=True, help="Agent labels JSON ([{id, type}, ...])")
    parser.add_argument("--output", required=True, help="Output corpus path")
    parser.add_argument("--no-balance", action="store_true", help="Skip balancing (keep all entries)")
    args = parser.parse_args()

    extracted = load_json(args.extracted)
    labels = load_json(args.labels)

    print(f"Extracted: {len(extracted)} entries")
    print(f"Labels: {len(labels)} entries")

    corpus = merge(extracted, labels)
    print(f"Merged: {len(corpus)} entries")

    if not args.no_balance:
        corpus = balance_corpus(corpus)

    if not corpus:
        print("No valid entries to write", file=sys.stderr)
        sys.exit(1)

    with open(args.output, "w") as f:
        json.dump(corpus, f, indent=2)

    think_count = sum(1 for e in corpus if e["type"] == "think")
    act_count = sum(1 for e in corpus if e["type"] == "act")
    print(f"\nCorpus written: {args.output}")
    print(f"  {think_count} think + {act_count} act = {len(corpus)} total")


if __name__ == "__main__":
    main()
