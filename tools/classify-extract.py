#!/usr/bin/env python3
"""
Extract unlabeled prompts from nmem DB for agent classification.

This is the first step in agent-driven corpus generation:
1. Run: python3 tools/classify-extract.py --limit 500 --output /tmp/unlabeled.json
2. Agent reads /tmp/unlabeled.json in batches of ~25
3. Agent classifies each as think/act, writes labels to /tmp/labels.json
4. Run: python3 tools/classify-ingest.py --extracted /tmp/unlabeled.json --labels /tmp/labels.json --output tools/corpus-think-act-N.json
5. Run: python3 tools/classify-train.py --corpus tools/corpus-think-act-N.json --output models/think-act.json

No API keys or local LLMs required — the agent itself is the labeler.

Usage:
    python3 tools/classify-extract.py --limit 500 --output /tmp/unlabeled.json
    python3 tools/classify-extract.py --limit 100 --min-length 30 --output /tmp/unlabeled.json
"""

import argparse
import json
import os
import random
import sqlite3
import sys


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


def extract_prompts(conn, limit, min_length):
    """Pull diverse prompts from DB — user prompts and agent thinking."""
    rows = []

    # User prompts — filter out system reminders and short entries
    cur = conn.execute(
        """SELECT p.id, 'user', p.content
           FROM prompts p
           WHERE p.source = 'user'
             AND LENGTH(p.content) > ?
             AND p.content NOT LIKE '<system-reminder>%'
             AND p.content NOT LIKE '%<system-reminder>%'
           ORDER BY RANDOM()
           LIMIT ?""",
        (min_length, limit // 2),
    )
    rows.extend(cur.fetchall())

    # Agent prompts (thinking blocks)
    cur = conn.execute(
        """SELECT p.id, 'agent', p.content
           FROM prompts p
           WHERE p.source = 'agent'
             AND LENGTH(p.content) > ?
           ORDER BY RANDOM()
           LIMIT ?""",
        (min_length, limit - len(rows)),
    )
    rows.extend(cur.fetchall())

    random.shuffle(rows)
    return rows


def main():
    parser = argparse.ArgumentParser(
        description="Extract unlabeled prompts from nmem DB for agent classification"
    )
    parser.add_argument("--limit", type=int, default=500, help="Number of prompts to extract")
    parser.add_argument("--min-length", type=int, default=20, help="Minimum content length")
    parser.add_argument("--output", default="/tmp/unlabeled.json", help="Output path")
    args = parser.parse_args()

    conn = open_db()
    rows = extract_prompts(conn, args.limit, args.min_length)
    conn.close()

    if not rows:
        print("No prompts found in DB", file=sys.stderr)
        sys.exit(1)

    entries = []
    for pid, source, content in rows:
        # Truncate to 500 chars for classification
        text = content[:500]
        entries.append({
            "id": pid,
            "source": source,
            "text": text,
        })

    with open(args.output, "w") as f:
        json.dump(entries, f, indent=2)

    user_count = sum(1 for e in entries if e["source"] == "user")
    agent_count = sum(1 for e in entries if e["source"] == "agent")
    print(f"Extracted {len(entries)} prompts ({user_count} user, {agent_count} agent)")
    print(f"Output: {args.output}")


if __name__ == "__main__":
    main()
