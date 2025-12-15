#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

LOG_FILE="/tmp/mcp-test-server.log"
echo "[mcp-test] starting dev MCP test server (logs: $LOG_FILE)..."
echo "[mcp-test] cmd: cargo run -p mcp-test-server -- --run-all-on-start true --open-ui true --serve-ui true" | tee "$LOG_FILE"
cargo run -p mcp-test-server -- --run-all-on-start true --open-ui true --serve-ui true >> "$LOG_FILE" 2>&1 &
SERVER_PID=$!
echo "[mcp-test] server pid $SERVER_PID"

cleanup() {
  echo "[mcp-test] stopping server pid $SERVER_PID"
  kill "$SERVER_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "[mcp-test] launching app with PLUGABLE_ENABLE_MCP_TEST=1"
PLUGABLE_ENABLE_MCP_TEST=1 npx tauri dev








