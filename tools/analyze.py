#!/usr/bin/env python3
"""Analyze nmem capture data — summarize event shapes and volumes.

Reads ~/.nmem/capture.jsonl and produces statistics.

Usage:
  python3 ~/workspace/nmem/tools/analyze.py
  python3 ~/workspace/nmem/tools/analyze.py --session    # per-session breakdown
  python3 ~/workspace/nmem/tools/analyze.py --raw        # dump all records
"""

import json
import sys
from collections import Counter, defaultdict
from pathlib import Path
from datetime import datetime

CAPTURE_FILE = Path.home() / ".nmem" / "capture.jsonl"


def load_records() -> list[dict]:
    if not CAPTURE_FILE.exists():
        print(f"No capture data at {CAPTURE_FILE}")
        sys.exit(1)
    records = []
    with open(CAPTURE_FILE) as f:
        for line in f:
            line = line.strip()
            if line:
                try:
                    records.append(json.loads(line))
                except json.JSONDecodeError:
                    continue
    return records


def fmt_bytes(n: int | float) -> str:
    if n < 1024:
        return f"{n:.0f} B"
    if n < 1024 * 1024:
        return f"{n / 1024:.1f} KB"
    return f"{n / (1024 * 1024):.1f} MB"


def summary(records: list[dict]):
    if not records:
        print("No records captured yet.")
        return

    # Time range
    ts_min = min(r["ts"] for r in records)
    ts_max = max(r["ts"] for r in records)
    duration_hrs = (ts_max - ts_min) / 3600

    print(f"=== nmem capture analysis ===")
    print(f"Records: {len(records)}")
    print(f"Time range: {datetime.fromtimestamp(ts_min):%Y-%m-%d %H:%M} — {datetime.fromtimestamp(ts_max):%H:%M} ({duration_hrs:.1f}h)")
    print()

    # Event type distribution
    print("--- Event distribution ---")
    event_counts = Counter(r["obs_type"] for r in records)
    for obs_type, count in event_counts.most_common():
        pct = count / len(records) * 100
        print(f"  {obs_type:20s} {count:5d}  ({pct:4.1f}%)")
    print()

    # Tool name distribution (PostToolUse only)
    tool_records = [r for r in records if r.get("tool_name")]
    if tool_records:
        print("--- Tool distribution ---")
        tool_counts = Counter(r["tool_name"] for r in tool_records)
        for tool, count in tool_counts.most_common():
            pct = count / len(tool_records) * 100
            print(f"  {tool:20s} {count:5d}  ({pct:4.1f}%)")
        print()

    # Size statistics
    print("--- Payload sizes ---")
    raw_sizes = [r["raw_payload_bytes"] for r in records]
    obs_sizes = [r["observation_bytes"] for r in records]
    input_sizes = [r["tool_input_bytes"] for r in records]
    response_sizes = [r["tool_response_bytes"] for r in records]
    content_sizes = [r["content_bytes"] for r in records]

    def stats(name, values):
        if not values:
            return
        avg = sum(values) / len(values)
        mx = max(values)
        total = sum(values)
        p50 = sorted(values)[len(values) // 2]
        p95 = sorted(values)[int(len(values) * 0.95)]
        print(f"  {name:20s}  avg={fmt_bytes(avg):>8s}  p50={fmt_bytes(p50):>8s}  p95={fmt_bytes(p95):>8s}  max={fmt_bytes(mx):>8s}  total={fmt_bytes(total):>8s}")

    stats("raw_payload", raw_sizes)
    stats("tool_input", input_sizes)
    stats("tool_response", response_sizes)
    stats("content (stored)", content_sizes)
    stats("observation (est)", obs_sizes)
    print()

    # Per obs_type size breakdown
    print("--- Per-type observation sizes ---")
    by_type = defaultdict(list)
    for r in records:
        by_type[r["obs_type"]].append(r["observation_bytes"])
    for obs_type, sizes in sorted(by_type.items(), key=lambda x: -len(x[1])):
        avg = sum(sizes) / len(sizes)
        total = sum(sizes)
        print(f"  {obs_type:20s}  n={len(sizes):4d}  avg={fmt_bytes(avg):>8s}  total={fmt_bytes(total):>8s}")
    print()

    # Volume projections
    print("--- Volume projections ---")
    total_obs_bytes = sum(obs_sizes)
    total_raw_bytes = sum(raw_sizes)
    events_per_hour = len(records) / max(duration_hrs, 0.01)
    obs_bytes_per_hour = total_obs_bytes / max(duration_hrs, 0.01)
    # Assume 6 active hours/day, 22 days/month
    monthly_events = events_per_hour * 6 * 22
    monthly_obs_bytes = obs_bytes_per_hour * 6 * 22
    # FTS5 roughly doubles storage
    monthly_with_fts = monthly_obs_bytes * 2

    print(f"  Events/hour:       {events_per_hour:.0f}")
    print(f"  Events/month (est): {monthly_events:,.0f}  (assuming 6h/day, 22 days)")
    print(f"  Events/year (est):  {monthly_events * 12:,.0f}")
    print(f"  Storage/month:     {fmt_bytes(monthly_with_fts)} (with FTS5)")
    print(f"  Storage/year:      {fmt_bytes(monthly_with_fts * 12)} (with FTS5)")
    print()

    # Discard ratio: raw payload vs what nmem stores
    compression = total_obs_bytes / max(total_raw_bytes, 1) * 100
    print(f"  Raw payload total: {fmt_bytes(total_raw_bytes)}")
    print(f"  Observation total: {fmt_bytes(total_obs_bytes)}")
    print(f"  Extraction ratio:  {compression:.1f}% of raw (structured extraction discards {100 - compression:.0f}%)")


def per_session(records: list[dict]):
    sessions = defaultdict(list)
    for r in records:
        sessions[r["session_id"]].append(r)

    print(f"=== Per-session breakdown ({len(sessions)} sessions) ===\n")
    for sid, recs in sorted(sessions.items(), key=lambda x: x[1][0]["ts"]):
        recs.sort(key=lambda r: r["ts"])
        ts_start = datetime.fromtimestamp(recs[0]["ts"])
        duration = (recs[-1]["ts"] - recs[0]["ts"]) / 60
        obs_bytes = sum(r["observation_bytes"] for r in recs)
        types = Counter(r["obs_type"] for r in recs)
        type_str = ", ".join(f"{t}:{c}" for t, c in types.most_common(5))
        print(f"  {sid}")
        print(f"    {ts_start:%Y-%m-%d %H:%M}  {duration:5.1f}min  {len(recs):4d} events  {fmt_bytes(obs_bytes):>8s}")
        print(f"    [{type_str}]")
        print()


def dump_raw(records: list[dict]):
    for r in records:
        ts = datetime.fromtimestamp(r["ts"])
        print(f"{ts:%H:%M:%S}  {r['obs_type']:20s}  {r.get('tool_name', ''):12s}  raw={fmt_bytes(r['raw_payload_bytes']):>8s}  obs={fmt_bytes(r['observation_bytes']):>8s}  {(r.get('file_path') or '')[:60]}")


def main():
    records = load_records()
    if "--raw" in sys.argv:
        dump_raw(records)
    elif "--session" in sys.argv:
        per_session(records)
    else:
        summary(records)


if __name__ == "__main__":
    main()
