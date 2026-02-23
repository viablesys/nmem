#!/bin/sh
# nmem hook wrapper — finds the nmem binary and passes through all args + stdin.
# Resolution order: $NMEM_BIN → $PATH → ~/.local/bin/nmem

set -e

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
