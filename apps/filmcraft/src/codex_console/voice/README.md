Voice transport uses WebRTC.rs 0.21 (MIT OR Apache-2.0) and the existing CPAL devices and
FilmCraft stream resampler. This prototype negotiates PCMU at 8 kHz (telephone bandwidth), not
Opus. Codec implementation reference: ITU-T G.711, November 1988, mu-law characteristic; RTP
mapping and byte order: RFC 3551, July 2003, section 4.5.14. No codec reference source was read or
copied. G.711 test oracles may run externally; no external encoder/decoder is linked or shipped.

Opt-in lifecycle qualification (signed-in Codex and Python 3 required): point
`FILMCRAFT_CONSOLE_TEST_CLI` to this directory's executable `fixture_mcp.py`, and
`FILMCRAFT_VOICE_TEST_PCM` to generated 8 kHz mono PCM16LE speech. Run the ignored
`codex_console::voice::recovery_live::expiry_cleanup_restarts_voice_on_same_thread_without_devices`
test. The fixture advertises the current Console catalog but refuses **every** tool call; this
qualifies voice lifecycle, not project inspection/editing. The test never opens devices. It
shortens only the generated-audio input's lifetime, exercises actual worker expiry, waits for
remote closure and local audio-worker release, restarts on the same conversation, then verifies
text still works. The production microphone lifetime remains one hour. This accelerated test
is not a one-hour endurance result.

Preventive Codex voice renewal is managed by the chat worker's monotonic policy in
`chat/renewal.rs`: warn at 54 minutes and begin a voice-only reconnect at 55 minutes.
The old voice connection must report closure, and a delegated voice task must confirm
cancellation/completion before audio restarts. An ordinary text task is not cancelled.
The same Codex thread, speaking voice, input channel and microphone mute are retained;
no prompt or edit is replayed. Explicit Stop, disconnect or Console close cancels the
renewal intent. Missing acknowledgements and startup failures remain bounded errors.
The microphone worker's one-hour lifetime is retained as a last-resort limit.

The policy/controller tests use generated clocks and protocol events, without devices
or a provider. They establish state-machine invariants, not real 55-minute endurance or
packaged behavior. Follow ROADMAP.md and the native acceptance records for release status.

Voice context continuity is separate from thread identity. A short disconnect/reconnect can
start a fresh realtime conversation even when Codex resumes the same backing thread. Voice
startup now supplies V3 `initialItems` with role-bearing recent dialogue and a partial project/
assistant-memory excerpt. The existing thread startup context remains enabled. Live transcript
deltas are accumulated and final transcripts replace their partial text; renewal and Voice
Off/On retain this history. A full Codex reconnect seeds it from the selected local conversation
archive. Explicit New Conversation and project/thread switches never import another conversation's
dialogue. Historical text is never submitted through a new `turn/start` or `appendText` request.

The handoff is deliberately partial: at most 32 dialogue items / 4096 UTF-8 bytes, 1024 bytes per
item, plus a 2048-byte memory excerpt and a fixed no-replay notice. This conservative text-byte
budget stays below Codex 0.166.0's advertised 128-item / 8192-estimated-token V3 startup limits.
Older history and full structured assistant memory remain with Codex / the memory tools. The
voice instructions require consulting Codex when those are needed, rather than treating the
excerpt as complete or treating historical completion claims as current project evidence.

Qualification must check actual phrase/role recall across a short Voice Off/On and a complete
Codex disconnect/resume, plus no spontaneous edit replay and project isolation. The generated
protocol tests establish handoff contents and bounds, not model recall. An isolated 2026-11-04
real text-process reconnect retained its test phrase; a V3 socket probe required separate API-key
authentication and was stopped without enabling that deferred route. The production WebRTC
controller/peer was then tested using locally generated speech and the refusal-only MCP fixture:
with empty startup history the replacement voice reported that the phrase was unavailable and
fell back to a generic role; with the handoff it replied “copper otter, concert editor” after a
short stop/restart. The final two-cycle run took 24.33 seconds, without microphone/speaker access.
One earlier pre-recall data-channel failure is retained as a transport limitation, not a recall
result. Evidence is in `target/codex-reconnect-context-20261104/`.

Run the ignored `short_reconnect_recalls_voice_only_phrase_without_devices` test with
`FILMCRAFT_CONSOLE_TEST_CLI` pointing to `fixture_mcp.py` and generated 8 kHz mono PCM16LE
teach/recall files in `FILMCRAFT_VOICE_CONTEXT_TEACH_PCM` and
`FILMCRAFT_VOICE_CONTEXT_RECALL_PCM`. The teach phrase is “copper otter”; the recall fixture asks
for that phrase and the seeded editing role. `FILMCRAFT_VOICE_CONTEXT_BASELINE=1` deliberately
removes the restart history for a failing comparison. This never enables paid API access.
Short packaged Voice Off/On and complete Console disconnect/resume are still attended acceptance
items; no hour-long wait is required to test this context defect.
