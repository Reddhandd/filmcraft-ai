# FilmCraft automation

The default MCP server retains the general engine and UI surface. Three opt-in CLI profiles
serve distinct clients without changing that default:

- `filmcraft-cli mcp --controller-console --bridge 127.0.0.1:PORT`: the desktop Command Console's
  `filmcraft-console/3` contract. `command_list` discovers the running app's engine and UI registry;
  `command_run` executes native commands directly. `command_batch {label, steps}` groups 1–64
  supported project edits atomically into one native Undo step. Routine text and spoken edits need
  no Apply/Reject. File I/O, analysis and UI controls run individually. This profile requires a
  running desktop app and the matching CLI; it does not offer a headless console session.
- `filmcraft-cli mcp --controller-read-only`: project/sequence inspection, command discovery,
  supported-request planning and current-frame preview. It exposes no generic command runner or
  synthetic UI input.
- `filmcraft-cli mcp --controller-edit`: the read tools plus bounded preview/commit operations for
  markers, clip moves/deletion, audio placement/mute/alignment, transitions, one-clip nesting,
  Performance Match, colour presets, video effects and supported effect parameter/keyframe edits.

The read-only and edit controller profiles accept `--project` / `--demo` for a headless session,
or `--bridge 127.0.0.1:PORT` for a running app. Bridge authentication uses the existing
`FILMCRAFT_CONTROL_TOKEN` mechanism. The bounded edit profile checks the app's
`controller.capabilities` before guarded bridge operations and refuses an app that lacks the
required operation. Its preview/commit rules below do not apply to direct console commands.

The console starts with `console_capabilities`, which supplies its versioned operating guide,
owned-analysis schemas and live project context. `console_analyze` runs native `music`,
`performance` or `passages` work in bounded background jobs. `console_read_analysis` returns
app-owned results and exact coordinate origins; applications use `analysisId`, not model-supplied
plans. Results expire or become stale after edits. Similarity and beat detections are candidate
evidence, not proof of corresponding musical phrases, boundaries, downbeats or first audible sound.
`console_memory` reads or updates bounded creative notes and checks for unsaved drafts and
concurrent disk changes. Connection and microphone controls remain the user's controls.

`request_plan` resolves supported text into explicit IDs and a named preview tool. Preview does
not edit. Present the exact preview and obtain approval before calling its matching commit tool.
Preview tokens use OS randomness, are single-use, expire after five minutes, and are limited to
64 pending tokens per operation. Commit validates the project revision and active sequence inside
the engine command, before any edit or trim-settling side effect, and verifies the resulting state.
Stale, expired or replayed tokens require a fresh preview. Edits use native undo steps.

Audio placement previews use the canonical sequence frame grid for both interval endpoints, so the
committed clip matches the times shown in the preview even at fractional frame rates. Marker
commits explicitly target Program; changing UI focus to Source cannot redirect an approved marker.
Existing effects remain unchanged when a standard effect is inserted before intrinsic effects.
Controller-only engine responses include the effect representation needed for exact verification;
ordinary effect and colour-preset command responses remain compatible.

Controller request lines are capped at 1 MiB. Plans validate IDs, locked/enabled tracks, operation
scope and supported values rather than accepting arbitrary engine/UI commands. The general MCP
server keeps its upstream implementation in `server.rs`; the preserved profiles live separately
in `controller.rs` and share the current authenticated bridge client.

### Uncertain bridge outcomes

Non-replayable requests start on a fresh connection, including edits and allocating/consuming PDF
tickets. The native server can close an idle inspection connection after two minutes; reusing it
for a later non-replayable request can otherwise cause an unknown outcome before a useful reply.
Only explicitly retry-safe inspections use the bounded idle pool. A new connection does not replay
an earlier request, and transport failure still never triggers an automatic mutation retry.

The shared bridge retries only an exact allowlist of retry-safe method/engine queries. Read-only
annotations alone are insufficient: even `console.bridge.memory` reads may synchronize project state
and are excluded. Once a mutating request may have been sent, a partial write, lost/malformed reply
or read timeout returns an actionable **unknown-outcome** error and discards that connection. A valid
`{"ok":false,"error":"timeout"}` UI-reply response is also uncertain: the queued request may execute
later. Other valid app rejections remain app errors. Never automatically repeat an edit, Undo,
import, analysis job or export after an unknown outcome; let the original request settle and inspect
the project, history or job list before deciding whether to issue a new mutation. Request IDs
correlate replies; they are not server-side deduplication keys.

### Validation scope

The 2026-10-29 separate-build console qualification at `18ac73c8` passed real-Codex import,
sequence/media authoring, playhead movement, timeline selection, direct grouped edits and native
Undo/Redo, full-song performance candidate analysis, native Beat Cuts/section application, first
save, an eight-second H.264/AAC export, reconnect, and actual Resume-picker use after app restart.
All 240 exported frames decoded. The original project and everyday installation were unchanged;
the test app closed and released its GUI lease. See [testing](../../docs/testing.md#local-codex-console-qualification)
for the evidence boundaries. This does not certify every registry command or musical match accuracy.

The user confirmed Codex voice worked in a later attended demo. A separate generated-speech test
also passed transcription, returned speech and same-conversation MCP inspection without opening
audio devices. The final editing qualification kept voice off. These console results do not certify
unrelated external controllers, other providers or a standalone voice service.
