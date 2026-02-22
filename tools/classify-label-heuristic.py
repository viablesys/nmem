#!/usr/bin/env python3
"""
Heuristic labeling for locus, novelty, and friction classifiers.
Reads observations from the nmem database and applies rule-based labels.

Usage:
    python3 tools/classify-label-heuristic.py --dimension locus --output tools/corpus-locus.json
    python3 tools/classify-label-heuristic.py --dimension novelty --output tools/corpus-novelty.json
    python3 tools/classify-label-heuristic.py --dimension friction --output tools/corpus-friction.json

Requires: pysqlcipher3 or sqlite3 (if DB is unencrypted / using NMEM_KEY)
"""

import argparse
import json
import os
import re
import sqlite3
import sys
from pathlib import Path


def get_db_path():
    """Resolve nmem database path."""
    if p := os.environ.get("NMEM_DB"):
        return p
    return os.path.expanduser("~/.nmem/nmem.db")


def get_db_key():
    """Resolve encryption key."""
    if k := os.environ.get("NMEM_KEY"):
        return k
    key_file = os.path.expanduser("~/.nmem/key")
    if os.path.exists(key_file):
        return Path(key_file).read_text().strip()
    return None


def connect_db():
    """Connect to the nmem database."""
    db_path = get_db_path()
    key = get_db_key()

    if key:
        try:
            from pysqlcipher3 import dbapi2 as sqlcipher
            conn = sqlcipher.connect(db_path)
            conn.execute(f"PRAGMA key = '{key}'")
            conn.execute("PRAGMA cipher_compatibility = 4")
        except ImportError:
            print("Warning: pysqlcipher3 not available, trying plain sqlite3", file=sys.stderr)
            conn = sqlite3.connect(db_path)
    else:
        conn = sqlite3.connect(db_path)

    return conn


def fetch_observations(conn, limit=5000):
    """Fetch observations with content for labeling."""
    cursor = conn.execute(
        """SELECT id, obs_type, content, file_path,
                  json_extract(metadata, '$.failed') as failed
           FROM observations
           WHERE content IS NOT NULL AND content != ''
           ORDER BY RANDOM()
           LIMIT ?""",
        (limit,),
    )
    return cursor.fetchall()


# --- Locus heuristics ---

EXTERNAL_OBS_TYPES = {"web_search", "web_fetch", "mcp_call", "github"}
INTERNAL_OBS_TYPES = {"file_read", "file_edit", "file_write", "search"}

EXTERNAL_PATTERNS = [
    r"https?://",
    r"web_?search",
    r"web_?fetch",
    r"npm\s+install",
    r"pip\s+install",
    r"cargo\s+install",
    r"git\s+push",
    r"git\s+clone",
    r"curl\s+",
    r"wget\s+",
    r"gh\s+(pr|issue|api)",
    r"docker\s+(pull|push)",
]
EXTERNAL_RE = re.compile("|".join(EXTERNAL_PATTERNS), re.IGNORECASE)

INTERNAL_PATTERNS = [
    r"cargo\s+(build|test|check|clippy|run)",
    r"git\s+(add|commit|diff|status|log)",
    r"src/",
    r"tests/",
    r"\.rs\b",
    r"\.py\b",
    r"\.toml\b",
]
INTERNAL_RE = re.compile("|".join(INTERNAL_PATTERNS), re.IGNORECASE)


def label_locus(obs_type, content, file_path, failed):
    if obs_type in EXTERNAL_OBS_TYPES:
        return "external"
    if obs_type in INTERNAL_OBS_TYPES:
        return "internal"
    if obs_type == "git_push":
        return "external"
    if obs_type == "git_commit":
        return "internal"

    ext_match = bool(EXTERNAL_RE.search(content))
    int_match = bool(INTERNAL_RE.search(content))

    if ext_match and not int_match:
        return "external"
    if int_match and not ext_match:
        return "internal"

    # Default by file_path presence
    if file_path:
        return "internal"
    return None  # ambiguous


# --- Novelty heuristics ---

ROUTINE_PATTERNS = [
    r"^cargo\s+(build|test|check|clippy|run)\b",
    r"^git\s+(add|commit|push|diff|status|log)\b",
    r"^ls\b",
    r"^cat\b",
    r"^mkdir\b",
    r"^rm\b",
    r"^cp\b",
    r"^mv\b",
    r"^echo\b",
    r"^npm\s+(run|test|build)\b",
    r"^python3?\s+-m\s+pytest",
]
ROUTINE_RE = re.compile("|".join(ROUTINE_PATTERNS), re.IGNORECASE)

NOVEL_PATTERNS = [
    r"error|Error|ERROR",
    r"failed|Failed|FAILED",
    r"traceback",
    r"panic",
    r"warning.*unused",
    r"investigate|debug|diagnose",
    r"why\s+(does|is|did|doesn)",
    r"how\s+to",
    r"search.*for",
]
NOVEL_RE = re.compile("|".join(NOVEL_PATTERNS))


def label_novelty(obs_type, content, file_path, failed):
    if obs_type in ("web_search", "web_fetch"):
        return "novel"

    content_short = content[:200]

    if obs_type == "command":
        if ROUTINE_RE.search(content_short):
            return "routine"
        if len(content) > 300:
            return "novel"  # long/complex commands

    if obs_type in ("git_commit", "git_push"):
        return "routine"

    if NOVEL_RE.search(content_short):
        return "novel"

    if obs_type in ("file_read", "file_edit", "file_write") and file_path:
        return "routine"

    return None  # ambiguous


# --- Friction heuristics ---

FRICTION_PATTERNS = [
    r"error\[",
    r"Error:",
    r"FAILED",
    r"failed",
    r"panic",
    r"traceback",
    r"warning\[",
    r"could not compile",
    r"cannot find",
    r"no such file",
    r"permission denied",
    r"timed? ?out",
    r"connection refused",
    r"exit code [1-9]",
    r"exit status [1-9]",
]
FRICTION_RE = re.compile("|".join(FRICTION_PATTERNS), re.IGNORECASE)

SMOOTH_OBS_TYPES = {"git_commit", "git_push"}


def label_friction(obs_type, content, file_path, failed):
    if failed:
        return "friction"
    if obs_type in SMOOTH_OBS_TYPES:
        return "smooth"

    if FRICTION_RE.search(content[:500]):
        return "friction"

    if obs_type in ("file_edit", "file_write", "file_read"):
        return "smooth"

    if obs_type == "command" and not FRICTION_RE.search(content):
        return "smooth"

    return None  # ambiguous


LABELERS = {
    "locus": label_locus,
    "novelty": label_novelty,
    "friction": label_friction,
}


def main():
    parser = argparse.ArgumentParser(description="Heuristic labeling for classifier training")
    parser.add_argument("--dimension", required=True, choices=["locus", "novelty", "friction"])
    parser.add_argument("--output", required=True, help="Output corpus JSON path")
    parser.add_argument("--limit", type=int, default=5000, help="Max observations to process")
    parser.add_argument("--min-length", type=int, default=10, help="Min content length")
    args = parser.parse_args()

    conn = connect_db()
    observations = fetch_observations(conn, args.limit)
    print(f"Fetched {len(observations)} observations")

    labeler = LABELERS[args.dimension]
    corpus = []
    skipped = 0
    label_counts = {}

    for obs_id, obs_type, content, file_path, failed in observations:
        if len(content) < args.min_length:
            skipped += 1
            continue

        label = labeler(obs_type, content, file_path, failed)
        if label is None:
            skipped += 1
            continue

        corpus.append({"text": content, "type": label})
        label_counts[label] = label_counts.get(label, 0) + 1

    counts_str = ", ".join(f"{v} {k}" for k, v in sorted(label_counts.items()))
    print(f"Labeled {len(corpus)} observations ({counts_str}), skipped {skipped}")

    os.makedirs(os.path.dirname(args.output) or ".", exist_ok=True)
    with open(args.output, "w") as f:
        json.dump(corpus, f, indent=None)

    print(f"Written to {args.output}")
    conn.close()


if __name__ == "__main__":
    main()
