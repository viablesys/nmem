#!/usr/bin/env python3
"""
Generate synthetic labeled corpus for think/act classification.

Pulls real prompts from nmem DB, classifies each with a frontier LLM (Haiku),
outputs balanced labeled JSON corpus.

Usage:
    python3 tools/classify-generate.py --limit 500
    python3 tools/classify-generate.py --limit 500 --model claude-haiku-4-5-20250929
    python3 tools/classify-generate.py --augment tools/corpus.json --count 200

Requires: ANTHROPIC_API_KEY env var.
"""

import argparse
import json
import os
import random
import sqlite3
import sys
import time
import urllib.request

ANTHROPIC_API = "https://api.anthropic.com/v1/messages"
DEFAULT_MODEL = "claude-haiku-4-5-20250929"

SYSTEM_PROMPT = """You classify coding session text as either "think" or "act".

- think: figuring out what to do — investigating, exploring, deciding, reviewing, diagnosing, asking questions, making observations, evaluating trade-offs, reading code to understand it, researching approaches
- act: doing the thing — implementing, executing instructions, committing, writing code/docs, fixing bugs, creating files, running tests, deploying, configuring, installing

Return ONLY a JSON object: {"type": "think" or "act", "confidence": 0.0-1.0}
No explanation, no markdown."""

AUGMENT_PROMPT = """You are generating training data for a think/act text classifier.

Given this example of a "{label}" text from a coding session:
---
{text}
---

Generate {count} diverse paraphrases that preserve the same classification ("{label}") but vary the wording, specificity, and style. Include both short (1 line) and longer (2-3 line) variants. Mix user prompts and agent reasoning styles.

Return ONLY a JSON array of strings, no explanation."""


def call_anthropic(model, system, user, max_tokens=128, timeout=30):
    api_key = os.environ.get("ANTHROPIC_API_KEY")
    if not api_key:
        print("Error: ANTHROPIC_API_KEY not set", file=sys.stderr)
        sys.exit(1)

    body = json.dumps({
        "model": model,
        "max_tokens": max_tokens,
        "system": system,
        "messages": [{"role": "user", "content": user}],
    }).encode()

    req = urllib.request.Request(
        ANTHROPIC_API,
        data=body,
        headers={
            "Content-Type": "application/json",
            "x-api-key": api_key,
            "anthropic-version": "2023-06-01",
        },
    )

    with urllib.request.urlopen(req, timeout=timeout) as resp:
        data = json.loads(resp.read())
        text = data["content"][0]["text"].strip()
        if text.startswith("```"):
            lines = text.split("\n")
            lines = [l for l in lines if not l.strip().startswith("```")]
            text = "\n".join(lines).strip()
        return text


def open_db():
    db_path = os.path.expanduser("~/.nmem/nmem.db")
    if not os.path.exists(db_path):
        print(f"DB not found: {db_path}", file=sys.stderr)
        sys.exit(1)
    conn = sqlite3.connect(db_path)
    key_file = os.path.expanduser("~/.nmem/key")
    if os.path.exists(key_file):
        with open(key_file) as f:
            key = f.read().strip()
        conn.execute(f"PRAGMA key = '{key}'")
    return conn


def extract_prompts(conn, limit):
    """Pull diverse prompts from DB — user prompts and agent thinking."""
    rows = []

    # User prompts
    cur = conn.execute(
        """SELECT p.id, 'user', p.content
           FROM prompts p
           WHERE p.source = 'user'
             AND LENGTH(p.content) > 15
             AND p.content NOT LIKE '<system-reminder>%'
           ORDER BY RANDOM()
           LIMIT ?""",
        (limit // 2,),
    )
    rows.extend(cur.fetchall())

    # Agent prompts (thinking blocks)
    cur = conn.execute(
        """SELECT p.id, 'agent', p.content
           FROM prompts p
           WHERE p.source = 'agent'
             AND LENGTH(p.content) > 15
           ORDER BY RANDOM()
           LIMIT ?""",
        (limit - len(rows),),
    )
    rows.extend(cur.fetchall())

    random.shuffle(rows)
    return rows


def classify_batch(model, rows, min_confidence=0.8):
    """Classify prompts with frontier LLM, filter by confidence."""
    results = []
    errors = 0

    for i, (pid, source, content) in enumerate(rows):
        text = content[:500]
        source_label = "agent reasoning" if source == "agent" else "user prompt"
        prefix = f"[{source}] " if source == "agent" else "[user] "
        user_msg = f'Classify this {source_label} text:\n\n---\n{text}\n---\n\nReturn JSON only.'

        try:
            raw = call_anthropic(model, SYSTEM_PROMPT, user_msg)
            result = json.loads(raw)
            label = result.get("type")
            conf = result.get("confidence", 0)

            if label not in ("think", "act") or conf < min_confidence:
                print(f"  [{i+1}/{len(rows)}] SKIP conf={conf:.2f} label={label}", file=sys.stderr)
                continue

            results.append({
                "id": pid,
                "text": prefix + text,
                "type": label,
                "confidence": conf,
            })

            preview = text[:60].replace("\n", " ")
            print(f"  [{i+1}/{len(rows)}] {label:<6} {conf:.2f}  {preview}")

        except Exception as e:
            errors += 1
            print(f"  [{i+1}/{len(rows)}] ERROR: {e}", file=sys.stderr)

        # Rate limit
        if i % 10 == 9:
            time.sleep(0.5)

    print(f"\nClassified: {len(results)}, Errors: {errors}, Skipped: {len(rows) - len(results) - errors}")
    return results


def balance_corpus(results):
    """Balance to equal think/act counts."""
    think = [r for r in results if r["type"] == "think"]
    act = [r for r in results if r["type"] == "act"]

    min_count = min(len(think), len(act))
    if min_count == 0:
        return results

    random.shuffle(think)
    random.shuffle(act)
    balanced = think[:min_count] + act[:min_count]
    random.shuffle(balanced)

    print(f"Balanced: {min_count} think + {min_count} act = {len(balanced)} total")
    return balanced


def augment_corpus(model, corpus_path, count_per_entry):
    """Generate paraphrases of existing corpus entries."""
    with open(corpus_path) as f:
        corpus = json.load(f)

    augmented = list(corpus)  # keep originals
    next_id = max(e["id"] for e in corpus) + 1

    for i, entry in enumerate(corpus):
        text = entry["text"]
        label = entry["type"]

        prompt = AUGMENT_PROMPT.format(
            label=label, text=text, count=count_per_entry
        )

        try:
            raw = call_anthropic(model, "", prompt, max_tokens=2048, timeout=60)
            variants = json.loads(raw)

            for v in variants:
                augmented.append({
                    "id": next_id,
                    "text": v if v.startswith("[") else f"[user] {v}",
                    "type": label,
                })
                next_id += 1

            print(f"  [{i+1}/{len(corpus)}] +{len(variants)} variants for {label}")

        except Exception as e:
            print(f"  [{i+1}/{len(corpus)}] ERROR: {e}", file=sys.stderr)

        if i % 5 == 4:
            time.sleep(1)

    return augmented


def main():
    parser = argparse.ArgumentParser(description="Generate synthetic think/act corpus")
    parser.add_argument("--limit", type=int, default=500, help="Number of prompts to pull from DB")
    parser.add_argument("--model", default=DEFAULT_MODEL, help="Anthropic model to use")
    parser.add_argument("--min-confidence", type=float, default=0.8, help="Min confidence threshold")
    parser.add_argument("--augment", help="Augment existing corpus file with paraphrases")
    parser.add_argument("--count", type=int, default=3, help="Paraphrases per entry (with --augment)")
    parser.add_argument("--output", help="Output path (default: auto-generated)")
    args = parser.parse_args()

    if args.augment:
        print(f"Augmenting {args.augment} with {args.count} variants per entry...")
        results = augment_corpus(args.model, args.augment, args.count)
    else:
        conn = open_db()
        rows = extract_prompts(conn, args.limit)
        conn.close()

        if not rows:
            print("No prompts found in DB", file=sys.stderr)
            sys.exit(1)

        print(f"Classifying {len(rows)} prompts with {args.model}...")
        results = classify_batch(args.model, rows, args.min_confidence)
        results = balance_corpus(results)

    # Strip confidence from output (not part of corpus schema)
    for r in results:
        r.pop("confidence", None)

    out_path = args.output or f"tools/corpus-synthetic-{len(results)}.json"
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nCorpus written: {out_path} ({len(results)} entries)")


if __name__ == "__main__":
    main()
