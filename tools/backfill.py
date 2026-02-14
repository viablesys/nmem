#!/usr/bin/env python3
"""Backfill capture.jsonl from Claude Code session transcript files.

Parses .jsonl transcript files and extracts tool_use events into the same
format that capture.py produces from live hooks.

Usage:
  python3 ~/workspace/nmem/tools/backfill.py <transcript.jsonl>
  python3 ~/workspace/nmem/tools/backfill.py  # finds most recent transcript
"""

import json
import sys
import os
from pathlib import Path
from datetime import datetime

CAPTURE_FILE = Path.home() / ".nmem" / "capture.jsonl"
PROJECTS_DIR = Path.home() / ".claude" / "projects"


def find_latest_transcript() -> Path | None:
    """Find the most recently modified transcript file."""
    candidates = list(PROJECTS_DIR.rglob("*.jsonl"))
    if not candidates:
        return None
    return max(candidates, key=lambda p: p.stat().st_mtime)


def classify_tool(name: str) -> str:
    """Map Claude Code tool names to nmem obs_types."""
    match name:
        case "Bash":
            return "command"  # can't distinguish error without response
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
    """Simulate nmem content extraction from tool_input."""
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
            return tool_input.get("description", "")
        case "WebFetch":
            return tool_input.get("url", "")
        case "WebSearch":
            return tool_input.get("query", "")
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


def process_transcript(path: Path) -> int:
    records = []

    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue

            # Extract tool_use from assistant messages
            if entry.get("type") != "assistant":
                continue

            message = entry.get("message", {})
            content_blocks = message.get("content", [])
            timestamp_str = entry.get("timestamp", "")
            session_id = entry.get("sessionId", "")
            cwd = entry.get("cwd", "")

            # Parse timestamp
            try:
                ts = datetime.fromisoformat(timestamp_str.replace("Z", "+00:00")).timestamp()
            except (ValueError, AttributeError):
                ts = 0

            for block in content_blocks:
                if block.get("type") != "tool_use":
                    continue

                tool_name = block.get("name", "")
                tool_input = block.get("input", {})
                tool_input_json = json.dumps(tool_input)

                content = extract_content(tool_name, tool_input)
                file_path = extract_file_path(tool_name, tool_input)
                obs_type = classify_tool(tool_name)

                observation_size = (
                    8 + 8
                    + len(session_id.encode("utf-8"))
                    + len(cwd.encode("utf-8"))
                    + len(obs_type.encode("utf-8"))
                    + len("PostToolUse".encode("utf-8"))
                    + len(tool_name.encode("utf-8"))
                    + len(content.encode("utf-8"))
                    + (len(file_path.encode("utf-8")) if file_path else 0)
                    + 50  # metadata overhead
                )

                record = {
                    "ts": ts,
                    "event": "PostToolUse",
                    "tool_name": tool_name,
                    "obs_type": obs_type,
                    "session_id": session_id,
                    "raw_payload_bytes": len(tool_input_json.encode("utf-8")),
                    "tool_input_bytes": len(tool_input_json.encode("utf-8")),
                    "tool_response_bytes": 0,  # not in transcript
                    "content_bytes": len(content.encode("utf-8")),
                    "file_path": file_path,
                    "observation_bytes": observation_size,
                }
                records.append(record)

            # Also capture user messages from the same transcript
            if entry.get("type") == "user":
                pass  # user messages don't have tool_use blocks

    # Also scan for user prompt entries
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue

            if entry.get("type") != "user":
                continue

            message = entry.get("message", {})
            content_blocks = message.get("content", [])
            timestamp_str = entry.get("timestamp", "")
            session_id = entry.get("sessionId", "")
            cwd = entry.get("cwd", "")

            try:
                ts = datetime.fromisoformat(timestamp_str.replace("Z", "+00:00")).timestamp()
            except (ValueError, AttributeError):
                ts = 0

            # Extract text from user message
            text_parts = []
            for block in content_blocks:
                if isinstance(block, str):
                    text_parts.append(block)
                elif isinstance(block, dict) and block.get("type") == "text":
                    text_parts.append(block.get("text", ""))
            prompt_text = " ".join(text_parts)[:500]

            if prompt_text and not prompt_text.startswith("<system-reminder>"):
                record = {
                    "ts": ts,
                    "event": "UserPromptSubmit",
                    "tool_name": None,
                    "obs_type": "user_prompt",
                    "session_id": session_id,
                    "raw_payload_bytes": len(prompt_text.encode("utf-8")),
                    "tool_input_bytes": 0,
                    "tool_response_bytes": 0,
                    "content_bytes": len(prompt_text.encode("utf-8")),
                    "file_path": None,
                    "observation_bytes": (
                        8 + 8
                        + len(session_id.encode("utf-8"))
                        + len(cwd.encode("utf-8"))
                        + len("user_prompt".encode("utf-8"))
                        + len("UserPromptSubmit".encode("utf-8"))
                        + len(prompt_text.encode("utf-8"))
                        + 50
                    ),
                }
                records.append(record)

    # Sort by timestamp and write
    records.sort(key=lambda r: r["ts"])

    CAPTURE_FILE.parent.mkdir(parents=True, exist_ok=True)
    with open(CAPTURE_FILE, "a") as f:
        for record in records:
            f.write(json.dumps(record) + "\n")

    return len(records)


def main():
    if len(sys.argv) > 1:
        path = Path(sys.argv[1])
    else:
        path = find_latest_transcript()
        if path is None:
            print("No transcript files found")
            sys.exit(1)
        print(f"Using: {path}")

    if not path.exists():
        print(f"File not found: {path}")
        sys.exit(1)

    count = process_transcript(path)
    print(f"Backfilled {count} records to {CAPTURE_FILE}")


if __name__ == "__main__":
    main()
