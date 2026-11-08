# Agents

FilmCraft is built to be driven by AI agents, and largely built by them. Part 1 covers using
FilmCraft from an agent. Part 2 covers developing FilmCraft as an agent.

# Part 1: driving FilmCraft

## 1. MCP server

`filmcraft-cli mcp` serves MCP on stdio. It has two modes:

| Mode | Command | What it drives |
|---|---|---|
| Headless | `filmcraft-cli mcp --demo` or `--project p.fcproj` (neither = empty project) | an in-process engine session, no window |
| Bridge | `filmcraft-cli mcp --bridge 127.0.0.1:9876` | the running desktop app started with `filmcraft --control 9876` |

Headless mode also takes `--data-dir DIR` (user presets). Any other option is rejected (exit 2):
there are no folder-confinement flags such as `--automation-read-root`, so the server can read
and write wherever the user can.

**Claude Code.** The repository's `.mcp.json` registers both servers (`filmcraft` = bridge,
`filmcraft-headless` = demo). Both point at `target/release/filmcraft-cli`, so build it first:

```sh
cargo build --release -p filmcraft-cli -p filmcraft
export FILMCRAFT_CONTROL_TOKEN="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
./target/release/filmcraft --control 9876 &      # only for the bridge server
claude                                           # approve the project MCP servers when asked
```

To register the server by hand, or for another MCP client, use the absolute binary path:

```sh
claude mcp add filmcraft-headless -- /abs/path/filmcraft/target/release/filmcraft-cli mcp --demo
```

```json
{ "mcpServers": { "filmcraft": { "command": "/abs/path/filmcraft-cli", "args": ["mcp", "--bridge", "127.0.0.1:9876"] } } }
```

### API request limits and recovery

**Disabled / unresolved:** direct API connections are unavailable in future builds by owner
request. Use Codex or Desktop ChatGPT. The following describes the preserved experimental
implementation, not an enabled workflow; see [status and limitations](api-connection-status.md).

The API provider bounds each user request to 24 tool rounds, then requests a final summary with
`tool_choice: "none"`. Completed native results remain in a clean conversation checkpoint;
continuing requires a new user message and never replays completed calls automatically. If the
summary cannot be fetched, FilmCraft reports the pause locally. Cancellation, revoked ownership,
and uncertain native outcomes still keep the interrupted-session guard: inspect project/history/jobs
before starting another conversation rather than repeating an edit blindly.

History has separate item and byte budgets. FilmCraft reserves space before admitting more native
operations, including room for tool results and the final response. At the conversation-history limit,
save reusable instructions with **Save Assistant** and use **New Conversation**. A history-limit
notice is not a billing or token-balance error. The API connection currently has no voice support.
Actual frame, Source and timeline PNG captures may be inspected; metadata alone does not establish
that audio was heard or continuous video was watched.

### Console controls

The Studio Console layout uses original symbol buttons with function names on hover and keyboard
focus. Existing automation IDs remain available. New conversation, resume, session lock and Undo
remain above the conversation; project memory stays beside Save Assistant. Conversation output is
above the composer, retaining native text selection and Copy/Paste. Model, speaking-voice and
microphone settings open with `console.settings.toggle`; `ui.inspect.console.settingsOpen` reports
that state. Voice controls and failures remain outside settings. Desktop ChatGPT uses its external
composer and audio controls. Manual commands and command diagnostics are under the collapsed
**Diagnostics & manual commands** section (`console.advanced.toggle`).

### In-app Command Console profile

The desktop console launches the matching bundled CLI with `mcp --controller-console --bridge
127.0.0.1:PORT` and an inherited control token. Its version is `filmcraft-console/5`; this profile
differs from the general MCP server and the separate bounded controller preview/commit profiles:

- `console_capabilities` supplies the versioned operating guide, owned-analysis schemas and limits.
- `project_inspect` and `sequence_inspect` return compact pages (32 rows by default, at most
  64 and 24 KiB). Follow `nextOffset` with `offset` and `expected_revision=revision`; restart
  after an edit. Sequence filters include `item`, `track`, `clip` and `kind` (`clips`, `tracks`,
  `markers`, `selection`, `effects`, `parameters`, `keyframes`). Effects require a clip;
  parameters also require a zero-based effect index; keyframes also require a parameter name.
  Read only relevant pages after each major operation and before the next clip. A first page
  is never evidence that omitted clips do not exist. `doc_inspect` contains first pages only.
  Names/paths are limited to 512 UTF-8 bytes; parameter values over 2 KiB are explicitly omitted.
  The corresponding native queries are `project.inspectPage` / `sequence.inspectPage`, using
  `expectedRevision` (camel case). Native full inspection remains available outside Console.
- Console results have a final serialized byte budget (48 KiB; frame images 384 KiB).
  `resultOmitted` preserves the original error status and reports that a result was omitted,
  not that a mutation failed. Never replay an edit to get a reply; inspect the current state.
  Legacy full inspection IDs dispatched through `command_run` use the compact page queries.
  Compact `ui_inspect` excludes chat history and full widget trees.
- `command_list` discovers the running app's engine and UI registry, with filtering and pagination.
  `enabled` reflects current selection; commands accepting explicit IDs may work without selection.
- `command_run {id, params}` executes native commands directly, including playhead movement,
  selection, import, editing, effects, saving and export. Routine edits need no Apply/Reject.
  Opening a native dialog is not evidence that its operation completed.
- `command_batch {label, steps}` groups 1–64 supported project edits into one atomic Undo action.
  A failed step rolls the group back. File I/O, analysis, asynchronous mask tracking and UI controls run individually.
- `console_analyze` runs native background `music`, `performance` or `passages` analysis.
  `console_read_analysis` pages beat results and returns exact coordinate origins. Applications
  accept app-owned `analysisId` values, not model-supplied plans; edits invalidate older results.
  Section and timing-anchor overrides use the native validators. Passage trims require actual
  start/middle/end phrase and boundary review; similarity scores cannot replace that review.
- `console_memory` reads/searches/updates up to 1 MiB of saved local notes. Session context uses
  bounded excerpts, not the entire enlarged store. Reads return `totalBytes`, `offset`, `nextOffset`,
  `scope`, `documentEpoch` and `memoryGeneration`; follow continuations using `offset`, `scope`,
  `document_epoch` and `memory_generation`. Read limits are 4–8192 UTF-8 bytes. Small default reads
  remain whole when the encoded reply fits; larger/escaped replies explicitly report truncation.
  `search` takes a literal case-sensitive `query` (1–256 bytes) and 1–8 hits, with bounded snippets.
  `append` remains limited to 4096 bytes. `replace` requires the full exact prior `expected_text`.
  `patch` replaces a passage of at most 8192 bytes using `offset`, exact `expected_text`, `text` and
  current scope/epoch/generation. Restart reads after a stale-snapshot error. Unsaved editor changes,
  detected disk conflicts and failed writes never authorize overwriting notes. Memory grants no
  permissions and is separate from project save, Undo and crash recovery.
  Unsaved manual notes block assistant writes and project replacement. Notes never grant permission.
- `console_references` inspects pending reference coverage, removes attachments, adds project/range
  metadata references and manages creative style drafts/library/selection. Arbitrary file attachment
  paths and preset import/export remain native user controls. Text/PDF/PNG references and style
  preferences grant no tool authority. See [formats, budgets and persistence](console-reference-inputs.md).

New conversations can be durable before the project is saved. FilmCraft keeps a local conversation
archive in its application data folder and project memory beside saved projects (`.fcproj.console`).
The first save carries the conversation forward. Connect resumes the active thread, or the latest saved thread after an app restart, loading the current project context;
Resume Codex Session opens a picker, including conversations started in an unsaved project. Codex
owns full thread history; FilmCraft caches recent display messages. Resuming a conversation inspects
the current project and does not reconstruct an unsaved timeline that was discarded.

Console connection controls and microphone capture remain user controls. Voice starts off. Existing
export destinations require explicit `overwrite:true` with user intent, or a new destination path.
Use native Undo/Redo for edits, and inspect job results before claiming an export completed.

The shared bridge retries only its exact allowlist of retry-safe reads. If a mutating request may have reached
the app and its write or reply fails, it reports an unknown outcome instead of replaying the edit
or starting a second job. Inspect the live project, history or jobs before issuing another mutation;
an absent reply is not evidence that nothing happened. Even a valid UI-reply `"error":"timeout"` can
precede the original edit executing; let it settle rather than automatically repeating it. Request
IDs correlate replies, not deduplicate commands. See [the control protocol](control-protocol.md) for recovery semantics and
[console qualification](testing.md#local-codex-console-qualification) for the latest real workflow
evidence and musical-accuracy limitations.

### Effect parameter replies

`effects.setParam` returns `{"keyframes": n, "param": id, "value": value}` on success.
The keyframe count is zero for a static parameter: this is a successful static edit,
not a failure. `param` identifies the edited parameter and `value` reports the accepted
value; a choice supplied by name is returned as its numeric option index. An animated
edit reports the value written at the requested time, rather than the static fallback.
For example, setting opacity `blend` to `"Screen"` returns
`{"keyframes": 0, "param": "blend", "value": 8}`.

### From an installed release

The release packages ship `filmcraft-cli` alongside the desktop app, so no build is needed:

| Install | CLI |
|---|---|
| Windows (MSI) | `C:\Program Files\FilmCraft\filmcraft-cli.exe` by default (wherever you installed it otherwise), not on `PATH` |
| Linux (deb, rpm) | `/usr/bin/filmcraft-cli` |
| macOS | the separate `filmcraft-cli-<version>-macos-<arch>.zip` release asset (the `.app` holds only the desktop app) |

```sh
# Windows, default install folder
claude mcp add filmcraft -- "C:\Program Files\FilmCraft\filmcraft-cli.exe" mcp
# Linux, or macOS with the CLI unzipped onto PATH
claude mcp add filmcraft -- filmcraft-cli mcp
```

### Tools

| Tool | Modes | Purpose |
|---|---|---|
| `command_list` | both | every command: id, label, menu, shortcut, params, enabled now (`filter`, `enabled_only`) |
| `command_run` | both | run a command `{id, params}`; edits are undoable |
| `command_batch` | both | run several commands in order `{steps: [{id, params}], stop_on_error}` → `{completed, failed, results}` |
| `doc_inspect` | both | the project tree and the active sequence in one call (`project_inspect` + `sequence_inspect`) |
| `render_preview` | both | same as `render_frame` |
| `project_inspect` | both | bins and items with ids, types, durations; active sequence |
| `sequence_inspect` | both | the active sequence: tracks, clips (`start` / `end` and `sourceIn` / `sourceOut` in ticks, frames, `speed`, `reverse`: the clip plays its `sourceIn` to `sourceOut` stretch backward, `gainDb`), effects, transitions, markers (name and `comment`), playhead, selection |
| `media_import` | both | import files by absolute path (`paths`, an array; optional `bin`, `image_sequence`) |
| `render_frame` | both | PNG of the program frame at `seconds` (headless renders; bridge screenshots the Program monitor). Read-only: the playhead and selection are left as they were |
| `ui_inspect` | bridge | UI state: tool, workspace, panels, zoom, playback, fps |
| `ui_elements` | bridge | on-screen interactive elements with id, label and rect (`prefix` filter) |
| `ui_click` | bridge | click by `id` or `x`,`y`; `button`, `count`, `modifiers` |
| `ui_drag` | bridge | press–move–release between points or element ids |
| `ui_key` | bridge | key or shortcut: `Space`, `Cmd+K`, `Shift+Delete` |
| `ui_type` | bridge | type text into the focused field |
| `ui_screenshot` | bridge | PNG of the window or one `panel` |
| `ui_control` | bridge | call any control-channel method directly (`ui.set`, `ui.scroll`, `ui.timeline.locate`, …) |

Typical loop: `project_inspect` / `sequence_inspect` → get ids → `command_run` → `render_frame` or
`ui_screenshot` → look at the result → `edit.undo` if it's wrong.

To copy a timeline transition, run `sequence.copyTransition {"transition": id}`. Paste it at one or
more cuts with `sequence.pasteTransition {"cuts": [{"clip": id, "edge": "in"}]}`; an `out` edge
also works. Without explicit cuts, paste uses selected transitions, selected edit points, or exact
playhead cuts on targeted tracks. Linked edit-point selections paste only onto tracks matching the
copied transition’s audio/video kind. Explicitly supplied cuts still reject a kind mismatch.
The effect, parameters, reverse direction and frame-based timing
travel together. A bulk paste is one undo step and leaves clips in place. Locked tracks, mismatched
audio/video cuts, insufficient clip span or missing source-media handles reject the entire paste
without editing the sequence. Shorten the transition or move a clip's In/Out point to leave enough
source frames around the cut. The transition clipboard is session state and is not saved in a
project file.

To edit an applied transition, run `sequence.setTransition {"transition": id, "effect": "wipe",
"frames": 12, "align": "center"}`. `effect` accepts a video or audio transition id or name
appropriate for the track. `duration` (ticks) and `start` (ticks) can be used for an exact span;
`start` and `align` are alternatives. Other optional fields are `params`, `reverse`, and `reset`.
The edit is one undo step. Timing edits reject spans without source handles; all edits reject a
locked track. A successful paste selects the new transition IDs so Effect Controls follows them.

### Conventions

The server follows a few conventions that make it predictable for agents. They are not
FilmCraft-specific: the sibling craft apps' MCP servers can follow the same ones, so an agent that has
driven one app can drive the others.

- **Core tools.** `command_list`, `command_run`, `command_batch`, `doc_inspect` and `render_preview`
  work in both modes. The FilmCraft tools above stay as they are.
- **Titles and annotations.** Every tool has a `title` and the four MCP hints (`readOnlyHint`,
  `destructiveHint`, `idempotentHint`, `openWorldHint`), so a client can let the read-only tools
  (lists, inspects, renders, screenshots) run without asking and ask before edits. When you add a
  tool, set them in its `#[tool(...)]` attribute; a test checks every listed tool.
- **Strict arguments.** An unknown tool argument is a JSON-RPC `-32602` error that names it and the
  accepted ones (`unknown argument "filtr" for command_list; expected: enabled_only, filter`), not a
  silently ignored key. `command_run.params` goes to the command unchanged.
- **Errors.** A failing command is an `isError` result with the engine's message. A line that is not
  JSON gets a `-32700` parse error (id `null`) and the server keeps serving. A command that panics is
  reported as `internal error: …`, and the session stays usable.
- **Resources.** `filmcraft://document` (the project tree and active sequence, as `doc_inspect`) and
  `filmcraft://commands` (the command catalog, as `command_list`), both `application/json`. For MCP
  2026-07-28 clients such as current Claude Code, list and read results carry the `ttlMs` and
  `cacheScope` hints that revision requires.

### Long exports: progress and cancellation

`command_run {"id": "file.exportMedia", "params": {…, "wait": true}}` in headless mode blocks until
the file is written. It uses the MCP progress and cancellation utilities, so a client can show
progress and stop it:

- The export runs as the same background job `jobs.list` shows. The session is locked only for short
  polls, so other requests (`ping`, `doc_inspect`, `jobs.list`, …) are answered while it encodes.
- `file.exportMedia` with `wait: true`, an export job in `jobs.list` and each `export.queue.list`
  item carry `offlineMedia`: the offline items the export drew the placeholder for (see §2b), `[]`
  when none. The export itself still succeeds; deciding what to do about it is up to the caller.
- Every job in `jobs.list`, and every encoding item of `export.queue.list`, carries `etaSeconds`: the
  time left at the job's speed over the last 15 seconds, `null` until a second of progress has been
  measured (a loudness pass, a seek) and once it is done. The app shows it as `37% · 2:05 left` in the
  status bar, the Export queue and the Progress panel.
- Jobs and queue items also expose `phase` (`processing` or `finalizing`; `null` for queue items
  without a job). This describes work, not success. Counters reaching their total do not prove the
  file is finished: flush, container writing and preview publication can remain. During finalization
  `etaSeconds` is `null` and the UI says Finalizing. Wait for the result/finished outcome; do not
  replay an export because its counter reached 100% or its ETA disappeared.
- With `"_meta": {"progressToken": T}` on the `tools/call`, the server sends
  `notifications/progress` `{progressToken: T, progress: <frames done>, total: <frames>, message:
  <job status>}` at most every 100 ms while frames advance. Without a token it sends none.
- `notifications/cancelled` for the request stops the encode at the next batch (`jobs.cancel`),
  deletes the partial output (the movie, or the frames and caption sidecar written so far) and sends
  no response, as the MCP cancellation utility asks.
- Without `wait` the command returns `{job, path, poll: "jobs.list", cancel: "jobs.cancel"}` at
  once: poll `jobs.list` for the job's `progress`, `etaSeconds`, `finished` and `result`, and stop it
  with `jobs.cancel` (`export.queue.list` lists the Export Queue only, not direct exports, #1544). Other exports (`file.exportFrame`, interchange formats, …) finish quickly and
  ignore the token.
- Bridge mode works the same way: the export runs as an app job (the app stays responsive and shows
  "Exporting… NN%" in its status bar) and the call returns when it is written, however long it
  takes. `filmcraft-cli --bridge … exec file.exportMedia … wait=true` blocks the same way.

## 2. Control channel

`filmcraft --control 9876` (or `FILMCRAFT_CONTROL_PORT=9876`) listens on `127.0.0.1` only. Send one
JSON request per line and get one JSON reply per line. Set a fresh `FILMCRAFT_CONTROL_TOKEN` for the
desktop session as above; the CLI / MCP bridge must inherit the same variable. Raw requests must
include it as `token` on every line (replace the placeholders below). Full setup and method table:
[control-protocol.md](control-protocol.md).

```jsonc
{"id":1,"token":"<FILMCRAFT_CONTROL_TOKEN>","method":"engine.commands"}
{"id":2,"token":"<FILMCRAFT_CONTROL_TOKEN>","method":"engine.execute","params":{"command":"file.openDemoProject"}}
{"id":3,"token":"<FILMCRAFT_CONTROL_TOKEN>","method":"engine.execute","params":{"command":"playhead.set","params":{"seconds":2}}}
{"id":4,"token":"<FILMCRAFT_CONTROL_TOKEN>","method":"engine.execute","params":{"command":"sequence.addEdit"}}
{"id":5,"token":"<FILMCRAFT_CONTROL_TOKEN>","method":"engine.execute","params":{"command":"effects.apply","params":{"effect":"Gaussian Blur"}}}
{"id":6,"token":"<FILMCRAFT_CONTROL_TOKEN>","method":"ui.elements","params":{"prefix":"tools."}}
{"id":7,"token":"<FILMCRAFT_CONTROL_TOKEN>","method":"ui.click","params":{"id":"tools.Razor"}}
{"id":8,"token":"<FILMCRAFT_CONTROL_TOKEN>","method":"ui.key","params":{"key":"Cmd+Z"}}
{"id":9,"token":"<FILMCRAFT_CONTROL_TOKEN>","method":"ui.screenshot","params":{"path":"/tmp/timeline.png","panel":"Timeline"}}
```

Replies look like `{"id":4,"ok":true,"result":{"cuts":2}}` or `{"id":7,"ok":false,"error":"…"}`.
Minimal client:

```python
import json, os, socket
s = socket.create_connection(("127.0.0.1", 9876)); f = s.makefile("rw")
def call(method, **params):
    f.write(json.dumps({"id": 1, "token": os.environ["FILMCRAFT_CONTROL_TOKEN"], "method": method, "params": params}) + "\n"); f.flush()
    return json.loads(f.readline())
print(call("engine.execute", command="sequence.inspect"))
```

Notes:

- Time is in ticks (254 016 000 000 per second). Commands also accept `seconds`, `frame` or
  `timecode`.
- A track parameter (`track`, `audioTrack`) is a track id or a name such as `"V1"` / `"A2"`.
  Leaving it out picks the command's default track; naming a track the sequence does not have is
  an "invalid parameters" error, never a different track.
  In `timeline.place`, `track` is where the picture goes and `audioTrack` where the sound goes;
  an audio track given as `track` places the sound alone, on that track.
  The clip lands on the frame grid: its start and its end (`time + duration`) each snap to the
  nearest frame.
- `sequence.inspect` reports `durationFrames` as the number of frames export renders: a sequence
  that ends partway through a frame counts that frame.
- `engine.execute` also runs UI-only commands (`tool.razor`, `playback.toggle`,
  `window.workspace.color`, `window.panel.<name>`).
- A command that works on the selection is disabled ("no clips selected", "select a clip in the
  Project panel") until something is selected, unless the call names its targets under a key the
  command documents (`clips` / `clip` for timeline clips, `items` / `item` for project items):
  `clip.replaceFromBin {"clips":[12],"item":4}` runs with nothing selected and leaves the
  selection alone. Only the selection condition is answered by the named targets; every other
  one (an open sequence, a filled clipboard, the right kind of clip) still applies, and ids of
  nothing do not count. `command_list` / `engine.commands` keep reporting the selection's state.
- Element ids come from the previous frame. If an element is missing right after a layout change,
  the app retries on later frames before giving up.
- `timeline.move` takes linked partners along while Linked Selection is on: moving a picture
  clip moves its sound by the same offset (pass `"linked": false` to move only the listed clips).
  It returns `{"moved": [ids]}`, every clip that moved.
- Without a window, `filmcraft-cli run script.jsonl` (lines of `{"id":"…","params":{…}}`) runs the
  same commands headlessly. See §2b for the rest of the CLI.

## 2b. Command-line interface

Every command is also one shell call away. Options go anywhere; output is JSON; exit status is 0 on
success, 1 when a command fails and 2 on a usage error. `filmcraft-cli help`, `filmcraft-cli --help`
and `filmcraft-cli -h` print the reference and exit successfully.
If the reader of stdout closes the pipe early (`filmcraft-cli commands | head`), the CLI drops the rest
of its output but still finishes the work, saves included, and the exit status still reports
failures; any other stdout write error is reported and makes the status 1.

```sh
filmcraft-cli commands razor                     # find ids (add --json for machine output)
filmcraft-cli describe timeline.razor            # one command: menu, shortcut, params, enabled now
filmcraft-cli --demo inspect sequence            # project / sequence as JSON
filmcraft-cli --project p.fcproj --save exec timeline.razor seconds=3.5
filmcraft-cli --project p.fcproj exec effects.apply '{"effect":"Gaussian Blur"}'
filmcraft-cli --project p.fcproj exec markers.exportCsv path=review.csv
filmcraft-cli --project p.fcproj --save import a.mov b.wav
filmcraft-cli --project p.fcproj export out.mp4  # format from the extension; waits for the job
filmcraft-cli --project p.fcproj export out --preset "YouTube 1080p Full HD" --start 0 --end 10
filmcraft-cli export --list-presets prores       # built-in + user presets (--data-dir for another library)
filmcraft-cli --project p.fcproj exec export.queue.add preset="Apple ProRes 422 HQ" path=renders/ start=true wait=true
echo '{"id":"file.newBin","params":{"name":"Selects"}}' | filmcraft-cli --project p.fcproj --save run -
filmcraft-cli --bridge 127.0.0.1:9876 exec window.workspace.color   # the running app
```

`key=value` values are parsed as JSON when they can be (`3.5`, `true`, `[1,2]`), otherwise taken as
strings; dotted keys nest (`color.r=1`). `run` prints one JSON line per command
(`{"line","id","ok","result"|"error"}`) and stops at the first failure unless `--keep-going`.
`--save` writes back to `--project`; `--save-as path` writes elsewhere.

Media that is offline when it is needed (the file is missing, was made offline, or can't be read)
renders as the "media not found" placeholder. `export` and `render` then name each such item on
stderr (`item 8 a.mp4 (missing): /path/a.mp4`) and exit 1 after writing the output, so a moved or
unmounted media folder doesn't pass as a good render. `--allow-offline` accepts the placeholder
frames: the warning stays and the exit status is 0. The export result lists the same items as
`offlineMedia` (`[{item, name, path, status, error}]`, `status` as in `media.status`: `missing`,
`offline` or `unreadable`), and `[]` when every clip it drew was online; check before exporting with
`exec media.status` or `exec media.findMissing`.

`markers.exportCsv {path}` exports all markers in the active sequence, including colours hidden
in the UI and markers outside In/Out, sorted by start then id. The UTF-8 CSV has a header and CRLF
records; quotes, commas and embedded newlines are escaped. Columns are Sequence, Marker ID, Name,
Comment, Kind, Color, Start Timecode, End Timecode, Duration Frames, Start Ticks, Duration Ticks and
Frame Rate (exact numerator/denominator). Color uses the user's display label from Settings ▸
Labels. Timecodes include the sequence's start timecode and
drop-frame setting; End is exclusive, and a point marker's End equals Start. Duration Frames is
rounded down; tick columns preserve sub-frame timing (254016000000 ticks per second).
An empty sequence writes just the header. User text starting with a spreadsheet formula character
(`=`, `+`, `-`, `@`, including after whitespace) gets a leading apostrophe in the exported cell.
The export leaves the project, dirty flag and undo history untouched, works with locked tracks,
and uses the host's file service (atomic
on desktop, a download on web). It exports sequence markers only, not clip/source markers;
reports are limited to 100000 markers and a conservative 16 MiB output budget. Invalid timing or a
failed write reports an error. The menu opens the normal save dialog; cancelling writes nothing.

## 3. Verifying UI work

For any UI change, look at the result:

1. `cargo run --release -p filmcraft -- --control 9876`
2. Drive the feature through the control channel or the MCP bridge: commands, then clicks and drags
   by automation id.
3. Assert on `ui.inspect`, `ui.elements` and `sequence.inspect`.
4. Take `ui.screenshot` (the whole window and the panel you changed) and open the PNG.

Screenshots you commit must follow [AGENTS.md](../AGENTS.md) §1: FilmCraft only, openly licensed
media, sidecar and `ATTRIBUTION.md` entry.

# Part 2: developing FilmCraft

## 4. Orientation, in this order

1. [AGENTS.md](../AGENTS.md): absolute rules (assets, clean-room, licences).
2. [CLAUDE.md](../CLAUDE.md): working instructions and non-negotiables.
3. [ROADMAP.md](../ROADMAP.md) for the summary, then [gaps.md](gaps.md) (the ranked shortfalls) first,
   [target-app-parity.md](target-app-parity.md) and [roadmap.md](roadmap.md) (milestones, current focus).
4. [architecture.md](architecture.md), then the README and tests of the crate you'll touch.
5. [contributing.md](contributing.md) (how to add things, gates) and [testing.md](testing.md).

Maintainers also keep a local planning folder, `plan/`. It is gitignored and not in the repository,
and holds the task-level plan, the status file and behaviour reference notes. If you don't have it,
everything you need to contribute is in the public docs above. Ask a maintainer for a task id.

## 5. Autonomous work loop

1. **Orient.** Pick the next task: the next unchecked task in the maintainer status file, or an open
   [gaps.md](gaps.md) entry. Prefer those gaps (stability, `.prproj`, camera media, measurement,
   hardware, Windows/Linux) over adding more checklist items. Read the relevant
   architecture section and crate README.
2. **Plan tests first.** Write down the acceptance test before writing code.
3. **Implement and test.**
4. **Verify.** Run all gates (`cargo xtask ci`). For UI work, run the app with `--control`, drive it
   and look at the screenshots (§3).
5. **Record.** Update the crate README (behaviour decisions, test results, limitations) and
   the progress docs when a milestone moves (ROADMAP.md, docs/roadmap.md, the relevant parity doc
   and gaps.md, with their timestamps). Commit as `M<n>.<k>: …`, then move on to the next task.

For parallel agents, use one git worktree and one `CARGO_TARGET_DIR` per agent, and keep each crate
with one owner ([contributing.md §5](contributing.md#5-parallel-work-several-agents-or-worktrees)).

## 6. Decision rules

Don't block on choices you can make yourself:

- **Behaviour.** Match the publicly documented and observable behaviour of professional editors
  (help pages, published shortcut lists, using the app). Never inspect application internals
  ([AGENTS.md](../AGENTS.md) §2). If behaviour is undocumented, choose what editors most commonly
  expect and record the choice in the crate README.
- **Ask a human only about:**
  - licences outside the allowlist;
  - spending money;
  - publishing or pushing;
  - personal data;
  - destructive git operations;
  - anything AGENTS.md leaves unclear ("when in doubt, leave it out").

## 7. Definition of Done

A feature is done when:

- [ ] its command(s) are registered with label, menu path, shortcut (if any) and a parameter doc;
- [ ] engine tests cover it, including undo/redo and the disabled case;
- [ ] the UI is wired: menu, panel control and shortcut;
- [ ] every new interactive widget has an automation id;
- [ ] you have driven it through the control channel and reviewed a screenshot;
- [ ] all gates pass (`cargo xtask ci`), and new assets have sidecars;
- [ ] the crate README and the progress docs (ROADMAP.md, docs/gaps.md, the parity docs) are updated where relevant;
- [ ] it is committed with its task id.

### Reporting progress honestly

ROADMAP.md and [target-app-parity.md](target-app-parity.md) keep two numbers: **feature breadth**
(does it exist?) and **ready for real work** (does it hold up?). When you update either:

- say how a number was obtained: *measured* (a test, a diff, a benchmark you ran) or *estimated*;
- count an approximation, a stub or a partly wired setting as such, never as done;
- record known bugs and limitations in the crate README and [gaps.md](gaps.md), not only
  what works;
- benchmarks state the machine, the load average and before/after on the same build;
- don't raise a percentage without evidence a reviewer can rerun.

### Console image delivery and user controls (contract 10)

`capture_timeline {max_side?,range?}` delivers actual visible Timeline PNG pixels plus bounded time/zoom/identity context. An optional exact-tick range must already be visible. Capture does not change selection, playhead, zoom, playback or waveform visibility. Report closed, hidden or loading waveforms as limitations; blank pixels do not establish silence. Waveform images show visual amplitude/timing, not audio listening or musical identity. Use native analysis for exact measurements. No screenshot file is retained after AI delivery.

`capture_source {max_side?}` captures only the currently visible Source monitor panel, distinct
from Program output. It does not open media, change source selection, move a playhead or start
playback. A closed/hidden panel returns an error; an empty monitor cannot establish anything about
unseen media. Delivery is inline PNG through the same bounded, disk-free pipeline. This is a native
FilmCraft tool, not permission to capture other applications or the whole desktop.

Microphone mute is an explicit user control: it silences input while keeping AI speech audible, even while Codex is busy. Output Copy preserves the transcript; Paste into Message populates the draft and never sends it. `console.voice.mute`, `console.chat.copy` and `console.chat.paste` are user controls and cannot be invoked by the AI through `command_run`.

Source checks do not establish packaged native acceptance: actual provider image interpretation, clipboard focus behavior and live microphone mute require separate qualification before release.
