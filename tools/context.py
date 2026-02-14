#!/usr/bin/env python3
"""nmem SessionStart context generator.

Reads SessionStart hook payload from stdin, queries ~/.nmem/nmem.db,
and outputs context for injection into the session preamble.

Generates:
  - Recent user intents with observation counts
  - Key files for the project (by access frequency)
  - Last session signature
  - Open threads (recent agent reasoning snippets)

Output format: JSON with additionalContext field.

Usage (hook config):
  python3 ~/workspace/nmem/tools/context.py
"""

import json
import sys
import time
import traceback
import urllib.request
from pathlib import Path

DB_PATH = Path.home() / ".nmem" / "nmem.db"
VLOGS_ENDPOINT = "http://localhost:9428/insert/jsonline"

# Limits for context generation (startup defaults)
MAX_INTENTS = 8
MAX_FILES = 6
MAX_THREADS = 3
MAX_INTENT_CHARS = 80
MAX_THREAD_CHARS = 120

# Expanded limits for compact/clear (agent lost its context window)
COMPACT_MAX_INTENTS = 12
COMPACT_MAX_FILES = 10
COMPACT_MAX_THREADS = 5
COMPACT_MAX_RECENT_OBS = 15


def log_error(error: str, context: dict | None = None):
    record = {"_msg": error, "service": "nmem-context", "level": "error"}
    if context:
        for k, v in context.items():
            if v is not None:
                record[k] = str(v)
    try:
        req = urllib.request.Request(
            VLOGS_ENDPOINT,
            data=json.dumps(record).encode("utf-8") + b"\n",
            headers={"Content-Type": "application/stream+json"},
            method="POST",
        )
        urllib.request.urlopen(req, timeout=2)
    except Exception:
        pass


def derive_project(cwd: str) -> str:
    if not cwd:
        return "unknown"
    p = Path(cwd)
    home = Path.home()
    try:
        rel = p.relative_to(home)
    except ValueError:
        return p.name or "unknown"
    parts = rel.parts
    if not parts:
        return "home"
    skip = {"workspace", "dev", "viablesys", "forge"}
    for part in parts:
        if part not in skip:
            return part
    return parts[-1]


def generate_context(project: str, source: str = "startup") -> str | None:
    import sqlite3

    if not DB_PATH.exists():
        return None

    is_recovery = source in ("compact", "clear")

    # Use expanded limits when recovering from context loss
    max_intents = COMPACT_MAX_INTENTS if is_recovery else MAX_INTENTS
    max_files = COMPACT_MAX_FILES if is_recovery else MAX_FILES
    max_threads = COMPACT_MAX_THREADS if is_recovery else MAX_THREADS

    conn = sqlite3.connect(str(DB_PATH), timeout=3)
    conn.execute("PRAGMA journal_mode = WAL")

    lines = []
    if is_recovery:
        lines.append(f"# [{project}] nmem context (post-{source})")
    else:
        lines.append(f"# [{project}] nmem context")
    lines.append("")

    # Recent user intents for this project
    rows = conn.execute("""
        SELECT p.content,
               datetime(p.timestamp, 'unixepoch', 'localtime') AS ts,
               (SELECT COUNT(*) FROM observations WHERE prompt_id = p.id) AS obs_count
        FROM prompts p
        JOIN sessions s ON s.id = p.session_id
        WHERE s.project = ? AND p.source = 'user'
        ORDER BY p.timestamp DESC
        LIMIT ?
    """, (project, max_intents)).fetchall()

    if rows:
        lines.append("## Recent Intents")
        for content, ts, obs_count in reversed(rows):
            text = content[:MAX_INTENT_CHARS].replace("\n", " ")
            lines.append(f"- [{ts}] \"{text}\" → {obs_count} actions")
        lines.append("")

    # Key files for this project
    rows = conn.execute("""
        SELECT o.file_path,
               COUNT(*) AS access_count,
               SUM(CASE WHEN o.obs_type = 'file_edit' THEN 1 ELSE 0 END) AS edits,
               SUM(CASE WHEN o.obs_type = 'file_read' THEN 1 ELSE 0 END) AS reads
        FROM observations o
        JOIN sessions s ON s.id = o.session_id
        WHERE s.project = ? AND o.file_path IS NOT NULL
        GROUP BY o.file_path
        ORDER BY access_count DESC
        LIMIT ?
    """, (project, max_files)).fetchall()

    if rows:
        lines.append("## Key Files")
        home = str(Path.home())
        for fpath, access, edits, reads in rows:
            display = fpath.replace(home, "~")
            lines.append(f"- {display} (R:{reads} E:{edits})")
        lines.append("")

    # Last session signature for this project
    row = conn.execute("""
        SELECT s.signature
        FROM sessions s
        WHERE s.project = ? AND s.signature IS NOT NULL
        ORDER BY s.started_at DESC
        LIMIT 1
    """, (project,)).fetchone()

    if row and row[0]:
        try:
            sig = json.loads(row[0])
            if sig:
                lines.append("## Last Session Signature")
                parts = [f"{t}:{n}" for t, n in sig[:5]]
                lines.append(f"- {', '.join(parts)}")
                lines.append("")
        except (json.JSONDecodeError, TypeError):
            pass

    # Effort: compaction count for this project
    compactions = conn.execute("""
        SELECT COUNT(*) FROM observations o
        JOIN sessions s ON s.id = o.session_id
        WHERE s.project = ? AND o.obs_type = 'session_compact'
    """, (project,)).fetchone()[0]

    if compactions > 0:
        lines.append(f"## Effort")
        lines.append(f"- {compactions} context compactions recorded")
        lines.append("")

    # Open threads: recent agent reasoning
    rows = conn.execute("""
        SELECT p.content
        FROM prompts p
        JOIN sessions s ON s.id = p.session_id
        WHERE s.project = ? AND p.source = 'agent'
        ORDER BY p.timestamp DESC
        LIMIT ?
    """, (project, max_threads)).fetchall()

    if rows:
        lines.append("## Open Threads")
        for (content,) in reversed(rows):
            text = content[:MAX_THREAD_CHARS].replace("\n", " ")
            lines.append(f"- \"{text}...\"")
        lines.append("")

    # On compact/clear: include recent observations from current session
    # so the agent can reconstruct what it was just doing
    if is_recovery:
        rows = conn.execute("""
            SELECT o.obs_type, o.tool_name, o.content,
                   datetime(o.timestamp, 'unixepoch', 'localtime') AS ts
            FROM observations o
            JOIN sessions s ON s.id = o.session_id
            WHERE s.project = ?
            ORDER BY o.timestamp DESC
            LIMIT ?
        """, (project, COMPACT_MAX_RECENT_OBS)).fetchall()

        if rows:
            lines.append("## Recent Actions (this session)")
            for obs_type, tool_name, content, ts in reversed(rows):
                tool = tool_name or obs_type
                text = content[:100].replace("\n", " ")
                lines.append(f"- [{ts}] {tool}: {text}")
            lines.append("")

    conn.close()

    # Only return if we have meaningful content beyond the header
    if len(lines) <= 2:
        return None

    return "\n".join(lines)


def main():
    try:
        raw = sys.stdin.read()
    except Exception:
        return

    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        return

    event = payload.get("hook_event_name", "")
    if event != "SessionStart":
        return

    cwd = payload.get("cwd", "")
    source = payload.get("source", "startup")
    project = derive_project(cwd)

    context = generate_context(project, source)
    if not context:
        return

    output = {
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    }
    print(json.dumps(output))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        log_error(
            traceback.format_exc(),
            {"hook": "nmem-context", "phase": "main"},
        )
