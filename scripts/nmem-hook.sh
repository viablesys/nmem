#!/bin/sh
# nmem hook wrapper — finds the nmem binary and passes through all args + stdin.
# Resolution order: $NMEM_BIN → $PATH → ~/.local/bin/nmem
#
# Dedup guard: when both a dev workspace (.claude-plugin/) and the installed
# user-scope plugin define hooks, Claude Code fires both for every event.
# flock ensures only one invocation runs — the loser exits silently.

set -e

LOCK="/tmp/nmem-hook.lock"
exec 9>"$LOCK"
flock -n 9 || exit 0

if [ -n "$NMEM_BIN" ] && [ -x "$NMEM_BIN" ]; then
  exec "$NMEM_BIN" "$@"
fi

if command -v nmem >/dev/null 2>&1; then
  exec nmem "$@"
fi

LOCAL_BIN="$HOME/.local/bin/nmem"
if [ -x "$LOCAL_BIN" ]; then
  exec "$LOCAL_BIN" "$@"
fi

echo "nmem: binary not found. Install with: curl -fsSL https://raw.githubusercontent.com/viablesys/nmem/main/scripts/install.sh | sh" >&2
exit 1
