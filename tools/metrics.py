#!/usr/bin/env python3
"""Push nmem capture data to VictoriaMetrics as time-series metrics.

Reads ~/.nmem/capture.jsonl and pushes per-event metrics with original
timestamps. Idempotent — re-running overwrites the same data points.

Usage:
  python3 ~/workspace/nmem/tools/metrics.py           # push all
  python3 ~/workspace/nmem/tools/metrics.py --live     # push only new (since last run)

Metrics pushed:
  nmem_event_bytes       — observation size per event (labels: obs_type, tool_name, session)
  nmem_raw_bytes         — raw payload size per event
  nmem_content_bytes     — extracted content size per event
  nmem_tool_response_bytes — tool_response size (discarded by extraction)
"""

import json
import sys
import urllib.request
from pathlib import Path
from collections import defaultdict

CAPTURE_FILE = Path.home() / ".nmem" / "capture.jsonl"
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

        labels = f'obs_type="{obs_type}",tool_name="{tool}",session="{session}",event="{event}"'

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


def main():
    live_mode = "--live" in sys.argv

    since = read_watermark() if live_mode else 0
    records = load_records(since_ts=since)

    if not records:
        print("No new records to push.")
        return

    lines = make_lines(records)
    total = len(lines)
    pushed = 0

    for i in range(0, total, BATCH_SIZE):
        batch = lines[i:i + BATCH_SIZE]
        if push_batch(batch):
            pushed += len(batch)

    max_ts = max(r["ts"] for r in records)
    write_watermark(max_ts)

    events = len(records)
    print(f"Pushed {events} events ({pushed} metric lines) to VictoriaMetrics")

    # Summary
    by_type = defaultdict(int)
    for r in records:
        by_type[r.get("obs_type", "unknown")] += 1
    for t, n in sorted(by_type.items(), key=lambda x: -x[1]):
        print(f"  {t}: {n}")


if __name__ == "__main__":
    main()
