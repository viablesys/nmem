#!/usr/bin/env python3
"""nmem event capture hook — logs hook event summaries for volume analysis.

Reads Claude Code hook JSON from stdin, extracts the fields nmem would store,
and appends a JSONL line to ~/.nmem/capture.jsonl.

Errors are pushed to VictoriaLogs via OTLP.

Usage (hook config):
  python3 ~/workspace/nmem/tools/capture.py
"""

import json
import sys
import time
import traceback
import urllib.request
from pathlib import Path

CAPTURE_FILE = Path.home() / ".nmem" / "capture.jsonl"
VLOGS_ENDPOINT = "http://localhost:9428/insert/jsonline"


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
    skip = {"workspace", "dev", "viablesys", "forge"}
    for part in parts:
        if part not in skip:
            return part
    return parts[-1]


def log_error(error: str, context: dict | None = None):
    """Push error to VictoriaLogs via native jsonline ingestion."""
    record = {
        "_msg": error,
        "service": "nmem-capture",
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
        pass  # if logging fails, don't cascade


def extract_content(event: str, tool_name: str | None, payload: dict) -> str:
    """Simulate nmem's structured extraction — what would the content field be?"""
    if event == "SessionStart":
        return payload.get("source", "startup")

    if event == "Stop":
        return "session_end"

    if event == "UserPromptSubmit":
        prompt = payload.get("prompt") or ""
        return prompt[:500]

    # PostToolUse
    tool_input = payload.get("tool_input") or {}

    match tool_name:
        case "Bash":
            cmd = tool_input.get("command") or ""
            return cmd[:500]
        case "Read":
            return tool_input.get("file_path") or ""
        case "Write":
            return tool_input.get("file_path") or ""
        case "Edit":
            return tool_input.get("file_path") or ""
        case "Grep":
            pattern = tool_input.get("pattern") or ""
            path = tool_input.get("path") or ""
            return f"{pattern} in {path}" if path else pattern
        case "Glob":
            return tool_input.get("pattern") or ""
        case "Task":
            return tool_input.get("description") or ""
        case "WebFetch":
            return tool_input.get("url") or ""
        case "WebSearch":
            return tool_input.get("query") or ""
        case _:
            return tool_name or event


def extract_file_path(tool_name: str | None, payload: dict) -> str | None:
    """Extract file_path if applicable."""
    tool_input = payload.get("tool_input") or {}
    match tool_name:
        case "Read" | "Write" | "Edit":
            return tool_input.get("file_path")
        case "Grep" | "Glob":
            return tool_input.get("path")
        case _:
            return None


def classify_obs_type(event: str, tool_name: str | None, payload: dict) -> str:
    """Classify observation type per ADR-002 Q4."""
    if event == "SessionStart":
        return "session_start"
    if event == "Stop":
        return "session_end"
    if event == "UserPromptSubmit":
        return "user_prompt"

    # PostToolUse
    tool_response = payload.get("tool_response") or ""

    match tool_name:
        case "Bash":
            resp = tool_response.lower() if isinstance(tool_response, str) else ""
            if "exit code" in resp or "error" in resp[:200]:
                return "command_error"
            return "command"
        case "Read":
            return "file_read"
        case "Write":
            return "file_write"
        case "Edit":
            return "file_edit"
        case "Grep" | "Glob":
            return "search"
        case _ if tool_name and ("mcp" in (tool_name or "").lower() or "__" in (tool_name or "")):
            return "mcp_call"
        case _:
            return f"tool_{tool_name}" if tool_name else event.lower()


def main():
    try:
        raw = sys.stdin.read()
    except Exception:
        return

    raw_size = len(raw.encode("utf-8"))

    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        return

    event = payload.get("hook_event_name", "unknown")
    tool_name = payload.get("tool_name")
    session_id = payload.get("session_id") or ""
    cwd = payload.get("cwd") or ""
    project = derive_project(cwd)
    source = payload.get("source") or ""

    # Sizes of key fields
    tool_input = payload.get("tool_input") or {}
    tool_response = payload.get("tool_response") or ""
    tool_input_size = len(json.dumps(tool_input).encode("utf-8"))
    tool_response_size = len(tool_response.encode("utf-8")) if isinstance(tool_response, str) else 0

    # Simulate nmem extraction
    content = extract_content(event, tool_name, payload)
    file_path = extract_file_path(tool_name, payload)
    obs_type = classify_obs_type(event, tool_name, payload)

    # What nmem would actually store (estimated bytes)
    observation_size = (
        8  # id (integer)
        + 8  # timestamp
        + len(session_id.encode("utf-8"))
        + len(cwd.encode("utf-8"))  # project (derived from cwd)
        + len(obs_type.encode("utf-8"))
        + len(event.encode("utf-8"))  # source_event
        + (len(tool_name.encode("utf-8")) if tool_name else 0)
        + len(content.encode("utf-8"))
        + (len(file_path.encode("utf-8")) if file_path else 0)
        + 50  # metadata overhead estimate
    )

    record = {
        "ts": time.time(),
        "event": event,
        "tool_name": tool_name,
        "obs_type": obs_type,
        "session_id": session_id,
        "project": project,
        "source": source,
        "raw_payload_bytes": raw_size,
        "tool_input_bytes": tool_input_size,
        "tool_response_bytes": tool_response_size,
        "content_bytes": len(content.encode("utf-8")),
        "file_path": file_path,
        "observation_bytes": observation_size,
    }

    CAPTURE_FILE.parent.mkdir(parents=True, exist_ok=True)
    with open(CAPTURE_FILE, "a") as f:
        f.write(json.dumps(record) + "\n")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        log_error(
            traceback.format_exc(),
            {"hook": "nmem-capture", "phase": "main"},
        )
