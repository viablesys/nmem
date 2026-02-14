#!/usr/bin/env python3
"""Prototype nmem schema with real session data.

Creates a SQLite database with the ADR-001 v3.0 schema (sessions, prompts,
observations + FTS5) and populates it from Claude Code session transcripts.

Usage:
  python3 tools/schema_proto.py                    # load all transcripts
  python3 tools/schema_proto.py --query "ADR"      # FTS5 search
  python3 tools/schema_proto.py --intent            # show prompt→observation links
  python3 tools/schema_proto.py --stats             # schema-level stats
  python3 tools/schema_proto.py --timeline SESSION  # session timeline with intent
"""

import json
import sqlite3
import sys
from collections import Counter
from datetime import datetime
from pathlib import Path

DB_PATH = Path.home() / ".nmem" / "proto.db"
PROJECTS_DIR = Path.home() / ".claude" / "projects"

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
    source      TEXT NOT NULL,      -- "user" or "agent"
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

CREATE INDEX IF NOT EXISTS idx_obs_dedup ON observations(session_id, obs_type, file_path, timestamp);
CREATE INDEX IF NOT EXISTS idx_obs_session ON observations(session_id, timestamp);
CREATE INDEX IF NOT EXISTS idx_obs_prompt ON observations(prompt_id);
CREATE INDEX IF NOT EXISTS idx_obs_type ON observations(obs_type);
CREATE INDEX IF NOT EXISTS idx_obs_file ON observations(file_path) WHERE file_path IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_prompts_session ON prompts(session_id, id);
"""

FTS_SCHEMA = """
CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
    content,
    content='observations',
    content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS observations_ai AFTER INSERT ON observations BEGIN
    INSERT INTO observations_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS observations_ad AFTER DELETE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, content)
        VALUES('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS observations_au AFTER UPDATE ON observations BEGIN
    INSERT INTO observations_fts(observations_fts, rowid, content)
        VALUES('delete', old.id, old.content);
    INSERT INTO observations_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE VIRTUAL TABLE IF NOT EXISTS prompts_fts USING fts5(
    content,
    content='prompts',
    content_rowid='id',
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


def init_db() -> sqlite3.Connection:
    DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(DB_PATH))
    conn.execute("PRAGMA journal_mode = WAL")
    conn.execute("PRAGMA synchronous = NORMAL")
    conn.execute("PRAGMA foreign_keys = ON")
    conn.executescript(SCHEMA)
    conn.executescript(FTS_SCHEMA)
    return conn


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
        case "TaskOutput":
            return "tool_taskoutput"
        case "AskUserQuestion":
            return "tool_askuserquestion"
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


def derive_project(cwd: str) -> str:
    """Derive project name from working directory."""
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
    # Skip common prefixes, return the project-level directory
    skip = {"workspace", "dev", "viablesys", "forge"}
    for i, part in enumerate(parts):
        if part not in skip:
            return part
    return parts[-1]


def parse_timestamp(ts_str: str) -> int:
    try:
        return int(datetime.fromisoformat(ts_str.replace("Z", "+00:00")).timestamp())
    except (ValueError, AttributeError):
        return 0


def load_transcript(conn: sqlite3.Connection, path: Path) -> dict:
    """Load a single transcript into the schema. Returns stats."""
    entries = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                entries.append(json.loads(line))
            except json.JSONDecodeError:
                continue

    if not entries:
        return {"prompts": 0, "observations": 0}

    # Determine session info from first entry
    session_id = None
    cwd = ""
    min_ts = float("inf")
    max_ts = 0

    for e in entries:
        sid = e.get("sessionId")
        if sid and not session_id:
            session_id = sid
        if e.get("cwd"):
            cwd = e["cwd"]
        ts_str = e.get("timestamp", "")
        ts = parse_timestamp(ts_str)
        if ts > 0:
            min_ts = min(min_ts, ts)
            max_ts = max(max_ts, ts)

    if not session_id:
        return {"prompts": 0, "observations": 0}

    project = derive_project(cwd)

    # Check if session already loaded
    existing = conn.execute("SELECT id FROM sessions WHERE id = ?", (session_id,)).fetchone()
    if existing:
        return {"prompts": 0, "observations": 0, "skipped": True}

    # Insert session
    conn.execute(
        "INSERT INTO sessions (id, project, started_at, ended_at) VALUES (?, ?, ?, ?)",
        (session_id, project, int(min_ts) if min_ts != float("inf") else 0, int(max_ts) if max_ts > 0 else None),
    )

    prompt_count = 0
    obs_count = 0
    current_prompt_id = None

    # Process entries in order — interleave user prompts and tool calls
    for entry in entries:
        ts = parse_timestamp(entry.get("timestamp", ""))
        entry_session = entry.get("sessionId", session_id)

        if entry.get("type") == "user":
            message = entry.get("message", {})
            content_blocks = message.get("content", [])

            text_parts = []
            for block in content_blocks:
                if isinstance(block, str):
                    text_parts.append(block)
                elif isinstance(block, dict) and block.get("type") == "text":
                    text_parts.append(block.get("text", ""))

            prompt_text = "".join(text_parts).strip()

            # Skip system reminders and empty prompts
            if not prompt_text or prompt_text.startswith("<system-reminder>"):
                continue

            prompt_text = prompt_text[:500]

            cursor = conn.execute(
                "INSERT INTO prompts (session_id, timestamp, source, content) VALUES (?, ?, ?, ?)",
                (session_id, ts, "user", prompt_text),
            )
            current_prompt_id = cursor.lastrowid
            prompt_count += 1

        elif entry.get("type") == "assistant":
            message = entry.get("message", {})
            content_blocks = message.get("content", [])

            for block in content_blocks:
                if block.get("type") == "thinking":
                    thinking_text = block.get("thinking", "").strip()
                    if thinking_text:
                        cursor = conn.execute(
                            "INSERT INTO prompts (session_id, timestamp, source, content) VALUES (?, ?, ?, ?)",
                            (session_id, ts, "agent", thinking_text[:2000]),
                        )
                        current_prompt_id = cursor.lastrowid
                        prompt_count += 1

                elif block.get("type") == "tool_use":
                    tool_name = block.get("name", "")
                    tool_input = block.get("input", {})

                    obs_type = classify_tool(tool_name)
                    content = extract_content(tool_name, tool_input)
                    file_path = extract_file_path(tool_name, tool_input)

                    conn.execute(
                        """INSERT INTO observations
                           (session_id, prompt_id, timestamp, obs_type, source_event,
                            tool_name, file_path, content)
                           VALUES (?, ?, ?, ?, ?, ?, ?, ?)""",
                        (session_id, current_prompt_id, ts, obs_type, "PostToolUse",
                         tool_name, file_path, content),
                    )
                    obs_count += 1

    # Compute session signature
    rows = conn.execute(
        "SELECT obs_type, COUNT(*) as n FROM observations WHERE session_id = ? GROUP BY obs_type ORDER BY n DESC",
        (session_id,),
    ).fetchall()
    signature = json.dumps([(r[0], r[1]) for r in rows])
    conn.execute("UPDATE sessions SET signature = ? WHERE id = ?", (signature, session_id))

    conn.commit()
    return {"prompts": prompt_count, "observations": obs_count}


def load_all(conn: sqlite3.Connection):
    """Load all available transcripts."""
    transcripts = list(PROJECTS_DIR.rglob("*.jsonl"))
    transcripts.sort(key=lambda p: p.stat().st_mtime)

    total_p = 0
    total_o = 0
    loaded = 0

    for path in transcripts:
        stats = load_transcript(conn, path)
        if stats.get("skipped"):
            continue
        if stats["observations"] > 0:
            loaded += 1
            total_p += stats["prompts"]
            total_o += stats["observations"]
            print(f"  {path.stem[:12]}  {stats['prompts']:3d} prompts  {stats['observations']:3d} observations")

    print(f"\nLoaded {loaded} sessions: {total_p} prompts, {total_o} observations")


def cmd_stats(conn: sqlite3.Connection):
    """Print schema-level statistics."""
    sessions = conn.execute("SELECT COUNT(*) FROM sessions").fetchone()[0]
    prompts = conn.execute("SELECT COUNT(*) FROM prompts").fetchone()[0]
    observations = conn.execute("SELECT COUNT(*) FROM observations").fetchone()[0]
    linked = conn.execute("SELECT COUNT(*) FROM observations WHERE prompt_id IS NOT NULL").fetchone()[0]
    unlinked = observations - linked

    user_prompts = conn.execute("SELECT COUNT(*) FROM prompts WHERE source = 'user'").fetchone()[0]
    agent_prompts = conn.execute("SELECT COUNT(*) FROM prompts WHERE source = 'agent'").fetchone()[0]

    print(f"Sessions:     {sessions}")
    print(f"Prompts:      {prompts}")
    print(f"  user:         {user_prompts}")
    print(f"  agent:        {agent_prompts}")
    print(f"Observations: {observations}")
    print(f"  with intent:  {linked} ({100*linked//observations if observations else 0}%)")
    print(f"  no intent:    {unlinked} ({100*unlinked//observations if observations else 0}%)")

    # DB file size
    db_size = DB_PATH.stat().st_size
    print(f"\nDB size:      {db_size:,} bytes ({db_size/1024:.1f} KB)")

    # Per-type counts
    print("\nObservations by type:")
    for row in conn.execute("SELECT obs_type, COUNT(*) FROM observations GROUP BY obs_type ORDER BY COUNT(*) DESC"):
        print(f"  {row[0]:25s} {row[1]:4d}")

    # Prompt length stats
    print("\nPrompt lengths:")
    row = conn.execute("SELECT AVG(LENGTH(content)), MAX(LENGTH(content)), MIN(LENGTH(content)) FROM prompts").fetchone()
    print(f"  avg: {row[0]:.0f} chars  max: {row[1]}  min: {row[2]}")

    # Observations per prompt
    print("\nObservations per prompt:")
    row = conn.execute("""
        SELECT AVG(cnt), MAX(cnt), MIN(cnt) FROM (
            SELECT prompt_id, COUNT(*) as cnt FROM observations
            WHERE prompt_id IS NOT NULL GROUP BY prompt_id
        )
    """).fetchone()
    if row[0]:
        print(f"  avg: {row[0]:.1f}  max: {row[1]}  min: {row[2]}")


def cmd_query(conn: sqlite3.Connection, term: str):
    """FTS5 search with intent context."""
    # Search both prompts and observations
    print(f"Results for '{term}':\n")
    found = False

    # Search prompts (user intent + agent reasoning)
    rows = conn.execute("""
        SELECT p.source, p.content, s.project,
               datetime(p.timestamp, 'unixepoch', 'localtime') AS ts,
               (SELECT COUNT(*) FROM observations WHERE prompt_id = p.id) as obs_count
        FROM prompts_fts f
        JOIN prompts p ON p.id = f.rowid
        JOIN sessions s ON p.session_id = s.id
        WHERE prompts_fts MATCH ?
        ORDER BY rank
        LIMIT 10
    """, (term,)).fetchall()
    if rows:
        found = True
        print(f"  --- Intents ({len(rows)} matches) ---\n")
        for source, content, project, ts, obs_count in rows:
            label = "user" if source == "user" else "agent"
            print(f"  [{ts}] {label} ({project}, {obs_count} obs)")
            print(f"    \"{content[:150]}\"")
            print()

    # Search observations
    rows = conn.execute("""
        SELECT o.obs_type, o.tool_name, o.content, o.file_path,
               p.content AS intent, p.source AS intent_source, s.project,
               datetime(o.timestamp, 'unixepoch', 'localtime') AS ts
        FROM observations_fts f
        JOIN observations o ON o.id = f.rowid
        LEFT JOIN prompts p ON o.prompt_id = p.id
        JOIN sessions s ON o.session_id = s.id
        WHERE observations_fts MATCH ?
        ORDER BY rank
        LIMIT 10
    """, (term,)).fetchall()
    if rows:
        found = True
        print(f"  --- Observations ({len(rows)} matches) ---\n")
        for obs_type, tool, content, fpath, intent, intent_source, project, ts in rows:
            print(f"  [{ts}] {obs_type} ({tool}) in {project}")
            if fpath:
                print(f"    file: {fpath}")
            print(f"    content: {content[:120]}")
            if intent:
                src = "user" if intent_source == "user" else "agent"
                print(f"    intent ({src}): \"{intent[:100]}\"")
            print()

    if not found:
        print(f"  No results for '{term}'")


def cmd_intent(conn: sqlite3.Connection):
    """Show prompt→observation groupings."""
    rows = conn.execute("""
        SELECT p.id, p.content, p.source, p.session_id,
               datetime(p.timestamp, 'unixepoch', 'localtime') AS ts,
               COUNT(o.id) as obs_count
        FROM prompts p
        LEFT JOIN observations o ON o.prompt_id = p.id
        WHERE p.source = 'user'
        GROUP BY p.id
        ORDER BY p.timestamp DESC
        LIMIT 20
    """).fetchall()

    for pid, content, source, sid, ts, obs_count in rows:
        print(f"[{ts}] ({obs_count} obs) \"{content[:80]}\"")
        # Show agent reasoning and observations under this prompt
        # Find agent prompts that follow this user prompt (before next user prompt)
        agent_intents = conn.execute("""
            SELECT content FROM prompts
            WHERE session_id = ? AND source = 'agent' AND id > ?
              AND id < COALESCE(
                  (SELECT MIN(id) FROM prompts WHERE session_id = ? AND source = 'user' AND id > ?),
                  999999999)
            ORDER BY id LIMIT 2
        """, (sid, pid, sid, pid)).fetchall()
        for (ai_content,) in agent_intents:
            print(f"  {'reasoning':15s} {'':10s} \"{ai_content[:70]}\"")

        obs = conn.execute("""
            SELECT obs_type, tool_name, file_path, content
            FROM observations WHERE prompt_id IN (
                SELECT id FROM prompts
                WHERE session_id = ? AND id >= ?
                  AND id < COALESCE(
                      (SELECT MIN(id) FROM prompts WHERE session_id = ? AND source = 'user' AND id > ?),
                      999999999)
            )
            ORDER BY timestamp
        """, (sid, pid, sid, pid)).fetchall()
        for obs_type, tool, fpath, ocontent in obs:
            label = fpath.split("/")[-1] if fpath else ocontent[:60]
            print(f"  {obs_type:15s} {tool or '':10s} {label}")
        print()


def cmd_timeline(conn: sqlite3.Connection, session_prefix: str):
    """Show session timeline with intent markers."""
    row = conn.execute(
        "SELECT id FROM sessions WHERE id LIKE ?", (session_prefix + "%",)
    ).fetchone()
    if not row:
        print(f"No session matching '{session_prefix}'")
        return

    session_id = row[0]
    print(f"Session: {session_id}\n")

    # Interleave prompts and observations by timestamp
    events = []

    for pid, ts, content in conn.execute(
        "SELECT id, timestamp, content FROM prompts WHERE session_id = ? ORDER BY timestamp",
        (session_id,),
    ):
        events.append((ts, "prompt", pid, content, None, None))

    for oid, ts, obs_type, tool, fpath, content, pid in conn.execute(
        """SELECT id, timestamp, obs_type, tool_name, file_path, content, prompt_id
           FROM observations WHERE session_id = ? ORDER BY timestamp""",
        (session_id,),
    ):
        events.append((ts, "obs", oid, content, obs_type, tool))

    events.sort(key=lambda e: (e[0], 0 if e[1] == "prompt" else 1))

    for ts, kind, eid, content, obs_type, tool in events:
        ts_str = datetime.fromtimestamp(ts).strftime("%H:%M:%S")
        if kind == "prompt":
            print(f"  {ts_str}  >>> \"{content[:80]}\"")
        else:
            label = content[:60] if content else ""
            print(f"  {ts_str}    {obs_type:15s} {tool or '':10s} {label}")


def main():
    conn = init_db()

    if "--stats" in sys.argv:
        cmd_stats(conn)
    elif "--query" in sys.argv:
        idx = sys.argv.index("--query")
        term = sys.argv[idx + 1] if idx + 1 < len(sys.argv) else ""
        cmd_query(conn, term)
    elif "--intent" in sys.argv:
        cmd_intent(conn)
    elif "--timeline" in sys.argv:
        idx = sys.argv.index("--timeline")
        prefix = sys.argv[idx + 1] if idx + 1 < len(sys.argv) else ""
        cmd_timeline(conn, prefix)
    elif "--rebuild-fts" in sys.argv:
        conn.execute("INSERT INTO observations_fts(observations_fts) VALUES('rebuild')")
        conn.commit()
        print("FTS index rebuilt.")
    else:
        # Default: load all transcripts
        load_all(conn)

    conn.close()


if __name__ == "__main__":
    main()
