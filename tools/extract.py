#!/usr/bin/env python3
"""nmem v2 extractor — structured hook event storage.

Reads Claude Code hook JSON from stdin, stores into ~/.nmem/nmem.db
using the three-table schema (sessions, prompts, observations).

Events handled:
  SessionStart      → create/update session record
  UserPromptSubmit  → insert prompt (source="user"), set as current intent
  PostToolUse       → scan transcript for new thinking blocks, insert observation
  Stop              → compute session signature, finalize session

Thinking blocks are extracted incrementally from the transcript on each
PostToolUse call, capturing agent reasoning as prompts (source="agent").

Usage (hook config):
  python3 ~/workspace/nmem/tools/extract.py
"""

import json
import sys
import time
import traceback
import urllib.request
from pathlib import Path

DB_PATH = Path.home() / ".nmem" / "nmem.db"
VLOGS_ENDPOINT = "http://localhost:9428/insert/jsonline"

SCHEMA = """
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT PRIMARY KEY,
    project     TEXT NOT NULL,
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    signature   TEXT,
    summary     TEXT
);

CREATE TABLE IF NOT EXISTS prompts (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    timestamp   INTEGER NOT NULL,
    source      TEXT NOT NULL,
    content     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS observations (
    id          INTEGER PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id),
    prompt_id   INTEGER REFERENCES prompts(id),
    timestamp   INTEGER NOT NULL,
    obs_type    TEXT NOT NULL,
    source_event TEXT NOT NULL,
    tool_name   TEXT,
    file_path   TEXT,
    content     TEXT NOT NULL,
    metadata    TEXT
);

CREATE TABLE IF NOT EXISTS _cursor (
    session_id  TEXT PRIMARY KEY,
    line_number INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_obs_session ON observations(session_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_obs_prompt ON observations(prompt_id);
CREATE INDEX IF NOT EXISTS idx_obs_type ON observations(obs_type);
CREATE INDEX IF NOT EXISTS idx_obs_file ON observations(file_path) WHERE file_path IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_prompts_session ON prompts(session_id, id);
"""

FTS_SCHEMA = """
CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
    content, content='observations', content_rowid='id',
    tokenize='porter unicode61'
);
CREATE TRIGGER IF NOT EXISTS observations_ai AFTER INSERT ON observations BEGIN
    INSERT INTO observations_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER IF NOT EXISTS observations_ad AFTER DELETE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, content)
        VALUES('delete', old.id, old.content);
END;

CREATE VIRTUAL TABLE IF NOT EXISTS prompts_fts USING fts5(
    content, content='prompts', content_rowid='id',
    tokenize='porter unicode61'
);
CREATE TRIGGER IF NOT EXISTS prompts_ai AFTER INSERT ON prompts BEGIN
    INSERT INTO prompts_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER IF NOT EXISTS prompts_ad AFTER DELETE ON prompts BEGIN
    INSERT INTO prompts_fts(prompts_fts, rowid, content)
        VALUES('delete', old.id, old.content);
END;
"""


def log_error(error: str, context: dict | None = None):
    record = {
        "_msg": error,
        "service": "nmem-extract",
        "level": "error",
    }
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


def get_db():
    import sqlite3

    DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(DB_PATH), timeout=5)
    conn.execute("PRAGMA journal_mode = WAL")
    conn.execute("PRAGMA synchronous = NORMAL")
    conn.execute("PRAGMA foreign_keys = ON")
    conn.executescript(SCHEMA)
    # FTS tables and triggers — use executescript to handle
    # multi-statement triggers (trigger bodies contain semicolons)
    try:
        conn.executescript(FTS_SCHEMA)
    except sqlite3.OperationalError:
        pass
    return conn


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


def ensure_session(conn, session_id: str, cwd: str, ts: int):
    existing = conn.execute(
        "SELECT id FROM sessions WHERE id = ?", (session_id,)
    ).fetchone()
    if not existing:
        conn.execute(
            "INSERT INTO sessions (id, project, started_at) VALUES (?, ?, ?)",
            (session_id, derive_project(cwd), ts),
        )


def get_current_prompt_id(conn, session_id: str) -> int | None:
    row = conn.execute(
        "SELECT id FROM prompts WHERE session_id = ? ORDER BY id DESC LIMIT 1",
        (session_id,),
    ).fetchone()
    return row[0] if row else None


def scan_transcript(conn, session_id: str, transcript_path: str, ts: int) -> int | None:
    """Read new transcript lines, extract thinking blocks as prompts.

    Returns the prompt_id of the most recent prompt (user or agent).
    """
    path = Path(transcript_path)
    if not path.exists():
        return get_current_prompt_id(conn, session_id)

    # Get cursor
    row = conn.execute(
        "SELECT line_number FROM _cursor WHERE session_id = ?", (session_id,)
    ).fetchone()
    cursor = row[0] if row else 0

    latest_prompt_id = get_current_prompt_id(conn, session_id)

    try:
        with open(path) as f:
            for i, line in enumerate(f):
                if i < cursor:
                    continue
                line = line.strip()
                if not line:
                    continue
                try:
                    entry = json.loads(line)
                except json.JSONDecodeError:
                    continue

                if entry.get("type") != "assistant":
                    continue

                content_blocks = entry.get("message", {}).get("content", [])
                for block in content_blocks:
                    if block.get("type") != "thinking":
                        continue
                    thinking = block.get("thinking", "").strip()
                    if not thinking:
                        continue
                    # Dedup: check if we already stored this exact thinking block
                    truncated = thinking[:2000]
                    existing = conn.execute(
                        "SELECT id FROM prompts WHERE session_id = ? AND source = 'agent' AND content = ?",
                        (session_id, truncated),
                    ).fetchone()
                    if existing:
                        latest_prompt_id = existing[0]
                        continue

                    c = conn.execute(
                        "INSERT INTO prompts (session_id, timestamp, source, content) VALUES (?, ?, ?, ?)",
                        (session_id, ts, "agent", truncated),
                    )
                    latest_prompt_id = c.lastrowid

        # Update cursor to end of file
        new_cursor = i + 1 if "i" in dir() else cursor
    except Exception:
        new_cursor = cursor

    conn.execute(
        "INSERT OR REPLACE INTO _cursor (session_id, line_number) VALUES (?, ?)",
        (session_id, new_cursor),
    )

    return latest_prompt_id


def classify_tool(name: str) -> str:
    match name:
        case "Bash":
            return "command"
        case "Read":
            return "file_read"
        case "Write":
            return "file_write"
        case "Edit":
            return "file_edit"
        case "Grep" | "Glob":
            return "search"
        case "Task":
            return "task_spawn"
        case "WebFetch":
            return "web_fetch"
        case "WebSearch":
            return "web_search"
        case _ if "__" in name:
            return "mcp_call"
        case _:
            return f"tool_{name.lower()}"


def extract_content(name: str, tool_input: dict) -> str:
    match name:
        case "Bash":
            return (tool_input.get("command") or "")[:500]
        case "Read":
            return tool_input.get("file_path", "")
        case "Write":
            return tool_input.get("file_path", "")
        case "Edit":
            return tool_input.get("file_path", "")
        case "Grep":
            pattern = tool_input.get("pattern", "")
            path = tool_input.get("path", "")
            return f"{pattern} in {path}" if path else pattern
        case "Glob":
            return tool_input.get("pattern", "")
        case "Task":
            return tool_input.get("description") or tool_input.get("prompt", "")[:200]
        case "WebFetch":
            return tool_input.get("url", "")
        case "WebSearch":
            return tool_input.get("query", "")
        case "AskUserQuestion":
            qs = tool_input.get("questions", [])
            return qs[0].get("question", "") if qs else name
        case _:
            return name


def extract_file_path(name: str, tool_input: dict) -> str | None:
    match name:
        case "Read" | "Write" | "Edit":
            return tool_input.get("file_path")
        case "Grep" | "Glob":
            return tool_input.get("path")
        case _:
            return None


def handle_session_start(conn, payload: dict):
    session_id = payload["session_id"]
    cwd = payload.get("cwd", "")
    source = payload.get("source", "startup")
    ts = int(time.time())
    ensure_session(conn, session_id, cwd, ts)

    # Track compaction and resume events as observations
    if source in ("compact", "resume", "clear"):
        conn.execute(
            """INSERT INTO observations
               (session_id, prompt_id, timestamp, obs_type, source_event,
                tool_name, file_path, content)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
            (session_id, get_current_prompt_id(conn, session_id), ts,
             f"session_{source}", "SessionStart", None, None, source),
        )

    conn.commit()


def handle_user_prompt(conn, payload: dict):
    session_id = payload["session_id"]
    cwd = payload.get("cwd", "")
    ts = int(time.time())
    prompt = payload.get("prompt", "")

    if not prompt or prompt.startswith("<system-reminder>"):
        return

    ensure_session(conn, session_id, cwd, ts)

    conn.execute(
        "INSERT INTO prompts (session_id, timestamp, source, content) VALUES (?, ?, ?, ?)",
        (session_id, ts, "user", prompt[:500]),
    )
    conn.commit()


def handle_post_tool_use(conn, payload: dict):
    session_id = payload["session_id"]
    cwd = payload.get("cwd", "")
    ts = int(time.time())
    tool_name = payload.get("tool_name", "")
    tool_input = payload.get("tool_input") or {}
    transcript_path = payload.get("transcript_path", "")

    ensure_session(conn, session_id, cwd, ts)

    # Scan transcript for new thinking blocks
    prompt_id = scan_transcript(conn, session_id, transcript_path, ts)

    # Extract observation
    obs_type = classify_tool(tool_name)
    content = extract_content(tool_name, tool_input)
    file_path = extract_file_path(tool_name, tool_input)

    conn.execute(
        """INSERT INTO observations
           (session_id, prompt_id, timestamp, obs_type, source_event,
            tool_name, file_path, content)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
        (session_id, prompt_id, ts, obs_type, "PostToolUse",
         tool_name, file_path, content),
    )
    conn.commit()


def handle_stop(conn, payload: dict):
    session_id = payload["session_id"]
    ts = int(time.time())
    transcript_path = payload.get("transcript_path", "")

    # Final transcript scan to catch any remaining thinking blocks
    scan_transcript(conn, session_id, transcript_path, ts)

    # Compute session signature
    rows = conn.execute(
        "SELECT obs_type, COUNT(*) FROM observations WHERE session_id = ? GROUP BY obs_type ORDER BY COUNT(*) DESC",
        (session_id,),
    ).fetchall()
    signature = json.dumps([(r[0], r[1]) for r in rows]) if rows else None

    conn.execute(
        "UPDATE sessions SET ended_at = ?, signature = ? WHERE id = ?",
        (ts, signature, session_id),
    )
    conn.commit()


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
    session_id = payload.get("session_id")
    if not session_id:
        return

    conn = get_db()
    try:
        match event:
            case "SessionStart":
                handle_session_start(conn, payload)
            case "UserPromptSubmit":
                handle_user_prompt(conn, payload)
            case "PostToolUse":
                handle_post_tool_use(conn, payload)
            case "Stop":
                handle_stop(conn, payload)
    finally:
        conn.close()


if __name__ == "__main__":
    try:
        main()
    except Exception:
        log_error(
            traceback.format_exc(),
            {"hook": "nmem-extract", "phase": "main"},
        )
