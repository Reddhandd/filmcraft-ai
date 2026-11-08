#!/usr/bin/env python3
"""Voice-only qualification fixture: advertise the contract, refuse every tool call.

This does not represent a live project or qualify editing. It lets opt-in voice lifecycle tests
use the production handshake without opening a GUI, microphone, speaker, or user project.
"""
import json
from pathlib import Path
import re
import sys

contract = (Path(__file__).resolve().parents[5] / "crates/engine/src/console_contract.rs").read_text()
section = contract.split("pub const CONSOLE_TOOLS: &[&str] = &[", 1)[1].split("];", 1)[0]
names = re.findall(r'"([a-z_]+)"', section)
if not names or len(names) > 64:
    raise SystemExit("Invalid Console fixture catalog")

for _ in range(10000):
    line = sys.stdin.buffer.readline(1024 * 1024 + 1)
    if not line or len(line) > 1024 * 1024:
        break
    try:
        value = json.loads(line)
    except (UnicodeError, ValueError):
        break
    if not isinstance(value, dict) or "id" not in value:
        continue
    number = value["id"]
    if not (type(number) is int or isinstance(number, str) and len(number) <= 256):
        break
    method = value.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
            "serverInfo": {"name": "FilmCraft voice-only refusal fixture", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": name,
            "description": "Unavailable in voice-only qualification; every tool call is refused.",
            "inputSchema": {"type": "object", "properties": {}},
        } for name in names]}
    elif method == "tools/call":
        result = {"isError": True, "content": [{
            "type": "text", "text": "Voice-only fixture refuses all tool calls; this is not a live project.",
        }]}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": number, "error": {"code": -32601, "message": "Voice-only fixture"}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": number, "result": result}), flush=True)
