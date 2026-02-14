#!/usr/bin/env python3
"""Push nmem metrics to VictoriaMetrics.

Two data sources:
  1. ~/.nmem/capture.jsonl  — per-event volume metrics (timestamped)
  2. ~/.nmem/nmem.db        — schema aggregate gauges (current state)

Usage:
  python3 ~/workspace/nmem/tools/metrics.py           # push all
  python3 ~/workspace/nmem/tools/metrics.py --live     # push only new (since last run)

Capture metrics (per-event, timestamped):
  nmem_event_bytes         — observation size per event
  nmem_raw_bytes           — raw payload size per event
  nmem_content_bytes       — extracted content size per event
  nmem_tool_response_bytes — tool_response size (discarded by extraction)

Schema metrics (gauges, current state):
  nmem_db_size_bytes       — database file size
  nmem_prompts_total       — prompt count by source and project
  nmem_observations_total  — observation count by obs_type and project
  nmem_sessions_total      — session count by project
  nmem_compactions_total   — compaction events by project
  nmem_active_sessions     — sessions without ended_at by project
"""

import json
import sys
import time
import urllib.request
from pathlib import Path
from collections import defaultdict

CAPTURE_FILE = Path.home() / ".nmem" / "capture.jsonl"
DB_PATH = Path.home() / ".nmem" / "nmem.db"
WATERMARK_FILE = Path.home() / ".nmem" / ".metrics_watermark"
VM_IMPORT = "http://localhost:8428/api/v1/import/prometheus"
BATCH_SIZE = 500


def load_records(since_ts: float = 0) -> list[dict]:
    if not CAPTURE_FILE.exists():
        print(f"No capture data at {CAPTURE_FILE}")
        sys.exit(1)
    records = []
    with open(CAPTURE_FILE) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                r = json.loads(line)
                if r["ts"] > since_ts:
                    records.append(r)
            except (json.JSONDecodeError, KeyError):
                continue
    return records


def sanitize_label(v: str | None) -> str:
    """Escape label values for Prometheus format."""
    if not v:
        return ""
    return v.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def make_lines(records: list[dict]) -> list[str]:
    """Convert capture records to Prometheus text format lines."""
    lines = []
    for r in records:
        ts_ms = int(r["ts"] * 1000)
        obs_type = sanitize_label(r.get("obs_type", "unknown"))
        tool = sanitize_label(r.get("tool_name") or "none")
        session = sanitize_label(r.get("session_id", "")[:12])
        event = sanitize_label(r.get("event", "unknown"))
        project = sanitize_label(r.get("project", "unknown"))
        source = sanitize_label(r.get("source", ""))

        labels = f'obs_type="{obs_type}",tool_name="{tool}",session="{session}",event="{event}",project="{project}",source="{source}"'

        lines.append(f'nmem_event_bytes{{{labels}}} {r.get("observation_bytes", 0)} {ts_ms}')
        lines.append(f'nmem_raw_bytes{{{labels}}} {r.get("raw_payload_bytes", 0)} {ts_ms}')
        lines.append(f'nmem_content_bytes{{{labels}}} {r.get("content_bytes", 0)} {ts_ms}')
        lines.append(f'nmem_tool_response_bytes{{{labels}}} {r.get("tool_response_bytes", 0)} {ts_ms}')

    return lines


def push_batch(lines: list[str]) -> bool:
    """Push a batch of Prometheus lines to VictoriaMetrics."""
    body = "\n".join(lines) + "\n"
    try:
        req = urllib.request.Request(
            VM_IMPORT,
            data=body.encode("utf-8"),
            method="POST",
        )
        urllib.request.urlopen(req, timeout=10)
        return True
    except Exception as e:
        print(f"Push failed: {e}")
        return False


def read_watermark() -> float:
    try:
        return float(WATERMARK_FILE.read_text().strip())
    except (FileNotFoundError, ValueError):
        return 0


def write_watermark(ts: float):
    WATERMARK_FILE.write_text(str(ts))


def make_schema_lines() -> list[str]:
    """Generate gauge metrics from nmem.db current state."""
    import sqlite3
    import os

    if not DB_PATH.exists():
        return []

    lines = []
    ts_ms = int(time.time() * 1000)

    # DB file size
    db_bytes = os.path.getsize(DB_PATH)
    lines.append(f'nmem_db_size_bytes {db_bytes} {ts_ms}')

    conn = sqlite3.connect(str(DB_PATH), timeout=3)
    conn.execute("PRAGMA journal_mode = WAL")

    # Prompts by source and project
    for source, project, count in conn.execute("""
        SELECT p.source, s.project, COUNT(*)
        FROM prompts p JOIN sessions s ON s.id = p.session_id
        GROUP BY p.source, s.project
    """):
        s = sanitize_label(source)
        p = sanitize_label(project)
        lines.append(f'nmem_prompts_total{{source="{s}",project="{p}"}} {count} {ts_ms}')

    # Observations by obs_type and project
    for obs_type, project, count in conn.execute("""
        SELECT o.obs_type, s.project, COUNT(*)
        FROM observations o JOIN sessions s ON s.id = o.session_id
        GROUP BY o.obs_type, s.project
    """):
        t = sanitize_label(obs_type)
        p = sanitize_label(project)
        lines.append(f'nmem_observations_total{{obs_type="{t}",project="{p}"}} {count} {ts_ms}')

    # Sessions by project
    for project, count in conn.execute("""
        SELECT project, COUNT(*) FROM sessions GROUP BY project
    """):
        p = sanitize_label(project)
        lines.append(f'nmem_sessions_total{{project="{p}"}} {count} {ts_ms}')

    # Active sessions (no ended_at)
    for project, count in conn.execute("""
        SELECT project, COUNT(*) FROM sessions
        WHERE ended_at IS NULL GROUP BY project
    """):
        p = sanitize_label(project)
        lines.append(f'nmem_active_sessions{{project="{p}"}} {count} {ts_ms}')

    # Compactions by project
    for project, count in conn.execute("""
        SELECT s.project, COUNT(*)
        FROM observations o JOIN sessions s ON s.id = o.session_id
        WHERE o.obs_type = 'session_compact'
        GROUP BY s.project
    """):
        p = sanitize_label(project)
        lines.append(f'nmem_compactions_total{{project="{p}"}} {count} {ts_ms}')

    conn.close()
    return lines


def main():
    live_mode = "--live" in sys.argv

    # Push capture-based metrics
    since = read_watermark() if live_mode else 0
    records = load_records(since_ts=since)
    capture_pushed = 0

    if records:
        lines = make_lines(records)
        for i in range(0, len(lines), BATCH_SIZE):
            batch = lines[i:i + BATCH_SIZE]
            if push_batch(batch):
                capture_pushed += len(batch)

        max_ts = max(r["ts"] for r in records)
        write_watermark(max_ts)

    # Push schema-based metrics
    schema_lines = make_schema_lines()
    schema_pushed = 0
    if schema_lines:
        if push_batch(schema_lines):
            schema_pushed = len(schema_lines)

    # Report
    events = len(records)
    print(f"Capture: {events} events ({capture_pushed} lines)")
    print(f"Schema:  {schema_pushed} gauge lines from nmem.db")

    if records:
        by_type = defaultdict(int)
        for r in records:
            by_type[r.get("obs_type", "unknown")] += 1
        for t, n in sorted(by_type.items(), key=lambda x: -x[1]):
            print(f"  {t}: {n}")


if __name__ == "__main__":
    main()
