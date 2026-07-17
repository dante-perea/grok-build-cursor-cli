#!/usr/bin/env bash
# Minimal ACP-shaped agent for RealGrokAgentDriver tests.
# Basename contains "fake-grok-agent" so the driver does not append `agent stdio`.
set -euo pipefail

# Drain stdin without hanging forever if the parent keeps the pipe open.
# Read up to 20 lines with 0.5s timeout each (bash read -t).
if [[ -n "${BASH_VERSION:-}" ]]; then
  for _ in $(seq 1 20); do
    IFS= read -r -t 0.5 line || break
  done
else
  # Fallback: best-effort drain
  cat >/dev/null 2>&1 || true
fi

# JSON-RPC responses (ids match driver handshake).
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"0.1.0","serverInfo":{"name":"fake-grok-agent","version":"0.0.1"},"capabilities":{}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"cursor-shell"}}'

# Streaming assistant + tool call with ACP Diff content (tools/edits path).
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"cursor-shell","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Applying a fix via search_replace.\n"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"cursor-shell","update":{"sessionUpdate":"tool_call","toolCallId":"tc-edit-fixture","title":"search_replace","status":"in_progress","rawInput":{"path":"src/fixture_edit.rs","old_string":"fn old() {}","new_string":"fn new() { println!(\"ok\"); }"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"cursor-shell","update":{"sessionUpdate":"tool_call","toolCallId":"tc-edit-fixture","title":"search_replace","status":"completed","content":[{"type":"diff","path":"src/fixture_edit.rs","oldText":"fn old() {}","newText":"fn new() { println!(\"ok\"); }"}]}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"cursor-shell","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Done — review the Diff Review pane."}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
exit 0
