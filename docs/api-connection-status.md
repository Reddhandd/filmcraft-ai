# Direct API connections: disabled / unresolved

Owner decision, 2026-11-01: focus on Codex and Desktop ChatGPT. Disable direct API connections in
future builds, retain the implementation and evidence, and defer further API development.

The production app does not construct an API adapter. The selector displays OpenAI API as disabled
and unresolved; the native provider-selection command rejects it before saving, disconnecting or
changing the current conversation. Existing Codex/Desktop state, drafts, projects, recovery and
Undo are preserved. Existing already-built demos are not changed by this source policy.

## Evidence and unresolved behavior

The signed `b1a36936` build passed native helper startup using a dummy key. An owner-entered real
key with `gpt-6-luna` successfully executed marker edits; the owner-clicked Console Undo restored
the generated sequence to its baseline, except the playhead. The exact named one-second test
prompt was not delivered on the Mac, so that instruction-following case is not claimed.

The subsequent three-frame test failed: the model reported no inspectable pixels for the earlier
one-second capture, described the eleven-second frame, and never captured twenty-one seconds.
The saved conversation deliberately contains pixel-free capture placeholders. Whether the failure
requires a different image-observation handoff remains unproved; it is not accepted as working.
No screenshot-file retention was authorized as a workaround. Continuous video streaming and API
voice are not implemented. A new provider such as Gemini Live is not in the approved scope.

Further API work and paid API acceptance tests are deferred, not alpha blockers. Re-enabling the
connection requires an explicit owner scope decision and renewed qualification. Existing API
journals and the source implementation are retained; credentials are not copied into documentation,
projects or Git. The latest test evidence is local and uncommitted under
`target/api-turn-budget-native-20261101/` (`api-startup-native-result.json`,
`api-live-undo-result.json`, `api-three-frame-diagnostics.json`).
