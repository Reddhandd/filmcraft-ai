# FilmCraft interface

The egui frontend draws panels and dialogs, and dispatches project changes through the engine.
Interactive controls keep stable automation ids regardless of the interface language.

## Control input limits

The control channel bounds `ui.click` to 10 clicks and `ui.drag` to 1024 movement steps (#2784).
The MCP bridge sends null for omitted counts/steps, so the validator treats null like an absent
field: one click or 12 drag steps. Negative, fractional, non-numeric and over-limit values still
return errors before queuing any input. Zero clicks emit no button events; 0–1 drag steps use 2.
`control::input_bounds_tests` covers defaults, rejection, unchanged pending input and valid
gestures; `keyboard_ui` verifies that rejected requests leave the UI usable. See the
[control protocol](../../docs/control-protocol.md).

## Transition drop previews

Dragging a video or audio transition over a compatible clip shows its actual time span, the
selected cut and an In/Out label. Shared cuts center the span; isolated edges use a one-sided
span. The preview uses the engine's read-only transition planner, including Timeline duration
preferences, and release dispatches `effects.apply` with the same edge. Locked tracks and
incompatible clip types show no valid transition preview. Ordinary effects retain their clip
outline. `timeline.transitionDropPreview` exposes the span and edge to automation.

`transition_drop_ui` covers both edges of short video/audio clips, hover without project edits,
the committed span and undo. Set `FILMCRAFT_UI_SNAPSHOT_DIR` to render screenshots with wgpu.

## Localisation

Edit > Language offers English, Japanese, Spanish, Brazilian Portuguese, Ukrainian, Simplified
Chinese, German, Russian, French, Indonesian, Korean and Italian (all but Ukrainian and Russian cover
the whole interface; those two cover the core menus). The language is stored in the engine's `general.interfaceLanguage` preference,
restored on startup, and also available in Settings > General.
Its default, System Language (`system`), follows the operating system: the first of the user's
preferred languages that the interface has (the host supplies them through
`HostHooks::system_languages`: `sys-locale` on the desktop, `navigator.languages` on the web),
otherwise English.
The `app.language.*` UI commands and `prefs.set` reach it through the control channel.
Japanese requires a craft-fonts build or a suitable installed font.

English source strings are lookup keys in `src/i18n/<code>.tsv`. `tl!` translates literals, `tlf!`
fills translated templates, and `i18n::t` translates names from registries. Placeholder values
(including user filenames containing braces) are inserted literally. Catalog translations are
original work using ordinary language, without proprietary localisation resources.

Spanish, Japanese, Simplified Chinese, French, Brazilian Portuguese and German cover menus, panels, dialogs, settings and registry labels. Searches accept both the
translated label and its English source, including Unicode capitals. Project content, command ids
and preference values retain their original values. Engine errors, CLI and MCP messages remain
English. Ukrainian and Russian currently cover the core menus; they fall back to English elsewhere.

A string a catalog lacks shows in English, so a missing translation never fails a test or a
build. `cargo xtask i18n-report` prints each language's missing strings (menus, `tl!` literals and
registry names) and its entries whose source the interface no longer has; `cargo xtask ci` prints
the same report in its log.

Verification: `cargo test -p filmcraft-ui-egui` checks catalog syntax, duplicate keys, placeholders
and UI behaviour in other languages (switching, search, font coverage); `cargo xtask ci` runs the
workspace gates. Visual checks use the control channel to switch language and capture the resulting
panels.

## Source playback

`source_playback.rs` auditions media independently of Program playback. It reuses MediaSource
decoding, output mapping, the device clock and the desktop play-ahead buffer. Only one monitor
owns the device at a time. Source frame jobs use the existing frame server and Source resolution.
The live project, undo stack and timeline playhead are unchanged. See [monitor documentation](../../docs/monitors.md)
for behavior, commands, limitations and regression tests. Source playback tests use synthetic
demo media and fake audio outputs; no third-party assets are introduced.

Source drag controls snapshot the selected span and dispatch `timeline.place` for video, audio,
or linked video/audio. Full-clip marks are implicit until set by the user. The engine owns edits,
range validation and undo; the UI owns the drag gesture. See `source_drag_ui` and the engine's
`source_placement_tests` for regression coverage.

Source range handles use transient gesture previews and the engine's integer-frame
`source_monitor::adjust_range`; release dispatches one mark edit. Monitor command routing
keeps Source navigation, markers and marked-range playback separate from Program.

## Pane layout controls

The panels use `craft-ui::docking` with its split-tree settings: craft-ui draws the frames, tab
strips, floating windows and drag previews and owns the gestures; `dock_ui.rs` draws the panel
bodies, the ≡ panel menu and the Timeline's sequence tabs. Bounded split geometry and
keyboard-accessible resize handles use `craft-ui::layout`. Panel tabs or their group's drag bar
move between tab groups, split at a group's or the window's edge, reorder, or float when released
with Cmd/Ctrl; a release over no target cancels. Tools and the Audio Meters keep their widths
as the window resizes (fixed split sizes, which a divider drag keeps fixed); the other groups
resize with the window. Every group resizes down to the same minimum and has no maximum.
Timeline sequence tabs retain their sequence-opening, closing and horizontal reorder behavior.

On desktop each floating panel opens in its own OS window. Tool panels are palettes: they stay
above other windows and hide while another application is active. The video viewers (Program,
Source, Reference Monitor, Lumetri Scopes) are normal windows, say on a second display: they
stack with the main window and stay shown in the background. A floating window moves by its
title bar, resizes from every edge and splits like the main window. While a modal dialog is open
(Settings, Recovery, Revert, a file dialog), the native windows are dimmed and take no input.
The main window's input rewrite (macOS Control+Click as a secondary click,
`--control-input-only`) also runs in each native window, and files dropped on one are imported
like files dropped on the main window; on the Timeline they land at the drop point. A drag from
a panel (a clip from the Source Monitor or the Project panel, an effect, a template) drops on a
panel in any of the app's windows. On Wayland
floating windows stay inside the main window. Where egui embeds viewports (tests), floating
windows stay inside the main window too.

Named workspaces save the layout, floating windows included; workspaces saved in the earlier
layout format still load. While every floating window is one tab group, a save also writes the
earlier format, so earlier versions still open the workspace. Automation IDs and workspace commands remain the integration boundary.

Tools and dock tabs expose their actual egui widget IDs, paint keyboard focus, and support
Tab/Shift-Tab traversal and Space/Enter activation. Control-injected keys can reach focused
widgets in a background window. The optional `--control-input-only` native launch mode isolates
automation from native widget/menu input; `ui.inspect` reports native and effective focus
separately. Focus and input-ownership regressions cover document/playhead preservation.

Compact monitor layouts separate the current and duration timecodes from zoom, resolution and
settings. Source video/audio drag choices retain their existing placement behavior. Transport
buttons use `craft-ui::layout::fit_toolbar`; commands beyond the available width remain available
through More, including their selected state and Source/Program routing. A scrolling fallback
retains all commands if even More cannot fit. `monitor_geometry` checks the painted readouts,
control bounds, overflow actions and Source drag payloads across monitor widths and themes.

## Preserved custom editing controls

Marquee Selection is a separate tool (`tool.marquee`, `tools.Marquee`). Dragging from a clip,
edge, volume line or transition selects clips without editing them. Shift or Command adds to
selection; Option selects individual clips without linked partners. Escape restores the original
selection. Its Timeline-scoped default is Control+Command+M on macOS and Shift+V on Windows/Linux;
M adds a marker in every panel. Existing modified shortcut sets are preserved. The Tools panel's `tools.shortcuts` button opens the shortcut
editor. Timeline Return invokes `playhead.returnToZero`; dialogs and text fields retain Return.
Option-middle-drag zooms about the playhead and Escape restores the initial view. Unmodified
middle-drag continues to pan. Close All Projects uses the dirty-project confirmation; the engine
currently supports one project per session.

## Local diagnostics and persistent history

Help ▸ Toggle Flight Recorder starts an opt-in rolling diagnostic log in the host data directory's
`Diagnostics` folder. Help ▸ Capture Diagnostic Report offers 30/60/120/600-second reports. Automation
uses `diagnostics.toggle` and `diagnostics.capture {seconds: 1..600}`. Reports include command IDs,
revision changes and performance counters, and omit command arguments and media paths. The buffer
is limited to 600 seconds / 12,000 events, and the live file compacts at 30 seconds or 4 MiB. Disk
write errors are reported and stop further disk appends. Monitoring is off on startup.

Settings ▸ General ▸ Save undo history with project (`general.persistentUndo`) is off by default.
When enabled, project saves also write an optional `.fcproj.undo` sidecar. Opening restores history
only if its base snapshot matches the loaded project; malformed, oversized or mismatched sidecars
produce a warning and leave the project open with fresh history. Sidecar failures never prevent a
project save. Histories are bounded to 200 entries per stack and 256 MiB on disk. Old sidecars whose
base cannot match the migrated project are rejected rather than guessed or silently applied.

Frame servers stop their idle workers when dropped and cancel queued/in-flight work. Worker-start
failures are reported; if no workers can start, the existing bounded UI pump handles frame jobs.
Waveform thread-start failures report an error and clear pending work so it can be retried.

## Local integration with current main

Transition paste follows linked edit-point selections on matching media tracks: a video transition
ignores the linked audio edit point. Explicit incompatible cuts and stale selected identities still
reject the whole paste. Copying clips, captions or a transition replaces the other clipboard kinds.
The transition clipboard and caption integration tests cover these interactions. Song-section name
fields expose their labels to assistive technology and retain their automation IDs.

## Command console (local development)

Window ▸ Command Console opens a session-only console. Enter an exact command ID followed by an
optional JSON object, for example `playhead.set {"seconds": 2}`. Prefix suggestions show command IDs
and labels; hover for parameter documentation and click to complete. Run dispatches through the
same UI/engine path as menus and the control channel. Results and errors appear in history; click
an entry to recall it. Undo uses the engine's existing history (queries and some UI commands are
not undoable). Clear History discards the console transcript.

Automation opens it with `window.commandConsole` and runs text with
`console.run {"line": "sequence.inspect"}`; `ui.inspect.console.open` reports visibility. Widgets use `console.input`, `console.run`,
`console.undo`, `console.clear`, `console.suggest.<command-id>` and `console.history.<index>`.
Malformed JSON, non-object parameters, oversized input and recursive console calls return errors.
Input is limited to 16 KiB, history to 64 entries and each result preview to 16 KiB. Direct command history
is excluded from serialized UI state, project files and preferences; it lasts only for this session.
The console does not invoke an operating-system shell.

The desktop console includes **Local Codex connection**, project memory and resumable chat. Build `filmcraft`
and `filmcraft-cli` into the same target directory, install Codex CLI (`codex` on PATH), and launch
the development app with the authenticated `--control PORT` setup in
[control-protocol.md](../../docs/control-protocol.md). Click **Connect Codex**, type a message and
click **Send**. Replies stream into the console; **Cancel** interrupts the current turn, including
when cancellation races with startup. **Disconnect Codex** stops the owned process. Closing the
console window alone keeps its connection alive. Sending messages uses your existing Codex
account/model configuration and shares the requested project context with that provider; sign in
through Codex CLI first. Connect starts a new conversation or reconnects the active one; Resume Codex Session opens the local conversation picker.

**Project memory and saved sessions.** Conversations can be durable before saving the project.
The application archive is `<data-dir>/Console/Conversations.console/sessions.json`; unsaved notes
use `<data-dir>/Console/Unsaved Project.console/memory.md`. First save carries the conversation
forward and migrates saved memory to `<project-path>.console/memory.md` without overwriting conflicts.
Existing project-local session indexes remain readable. These companion files load when using the
desktop console, not on ordinary project opening or hosts without a Codex connection. They are
separate from the project's editing data. **View/Edit Memory** opens an editable memory file with preferences,
decisions, current goal and where work stopped. **Save Memory** publishes changes atomically.
The current file is loaded automatically on every **New Conversation** and **Resume Codex
Session**, including edits made outside FilmCraft. Save draft memory before switching conversations. **Reload from Disk** discards draft memory,
reloads the companion files and disconnects Codex; use it after repairing files or resolving an
external-write conflict.

**Resume Codex Session** lists local conversations, including those begun before saving, with titles and UTC update dates.
Selecting one resumes the durable Codex thread and restores its cached recent chat and model
selection. Codex retains the full persisted conversation context; FilmCraft's display cache shows
at most the latest 32 messages. Every new/resumed conversation begins with a fresh tool-grounded
project inspection. Past proposals and approvals are never saved or replayed, and voice starts
off. Finish/cancel the active turn and stop voice before switching. Project replacement refreshes
context before further edits. Resuming chat does not reconstruct a discarded unsaved timeline.

Memory is reference context, not permission to execute edits or override the console's tool policy.
Memory is capped at 16 KiB; the application archive is capped at 8 MiB and 32 conversations, with
bounded message fields. Corrupt, oversized, conflicting or unwritable files report errors rather
than silently resetting history. Concurrent writes use an exclusive short-lived `write.lock` and
check for external index changes. A lock left by a terminated writer must be removed after checking
that no other FilmCraft instance is using the project. Preserve the project companion memory,
application archive and Codex's full durable thread storage when moving work. These files contain
private context, not the FilmCraft control token. Unsaved manual notes block assistant writes and
project replacement/close. `console_memory` reads or updates saved creative notes with scope and
concurrency checks; it does not grant permission to execute edits.

Automation commands: `console.session.new {}`, `console.sessions.open {}`,
`console.session.resume {threadId: id}`, `console.memory.open {}` and
`console.memory.save {text: memory}` and `console.sessions.reload {}`. Controls have corresponding IDs; the memory editor uses
`console.memory.input` and each saved-conversation row uses `console.session.<thread-id>`.
`ui.inspect.console` reports the memory path, active thread and saved-session metadata.

Project, sequence, selection and playhead grounding reuse
`filmcraft-cli mcp --controller-console --bridge 127.0.0.1:PORT` (`filmcraft-console/3`). The console explicitly disables
inherited MCP servers and plugins for each new or resumed thread, disables shell tools, hooks, apps,
web search and multi-agent operation, uses a read-only sandbox, and verifies the resulting tool
catalog before accepting prompts. Only configuration names are read for isolation: display config
values are not round-tripped into the new transport. The control credential stays in inherited
environment and authenticated loopback requests; it is never placed in arguments or config files.
Global Codex configuration and credentials are not edited or copied. Tested against Codex CLI
0.164.0; the dynamic-tool protocol requires experimental API capability negotiation. Incompatible
versions, auth failures, closed pipes, malformed messages and queue limits produce actionable errors.

`console_capabilities` supplies the versioned operating guide and owned-analysis schemas.
`command_list` discovers the live engine/UI registry with filtering, pagination and enablement;
`command_run` executes native commands, including import, sequence creation/placement, playhead,
selection, editing, effects, save and export. Routine edits need no Apply/Reject.
`command_batch {label, steps}` groups 1–64 supported project edits atomically into one normal Undo action;
failure restores the prior project/history. File I/O, analysis and UI actions run individually.
A dialog-opening result does not establish that its operation completed. The legacy native
`filmcraft_propose_edit` compatibility function also executes directly. Separate bounded controller
profiles retain their own preview/commit rules.

The command tools have scoped approval configuration while the model shell remains read-only.
The assistant cannot control its connection, start microphone capture or invoke external help/update
controls. Dirty project replacement is refused until saved; existing export/unrelated save paths
require explicit overwrite intent or a new destination. Native edits use ordinary Undo/Redo.
The shared bridge retries only its exact allowlist of retry-safe reads. An uncertain mutation
returns an unknown-outcome error without replay; inspect project/history/jobs before a fresh request.
A valid UI-reply `"error":"timeout"` can precede the queued edit executing; let that request settle
instead of automatically repeating it. Request IDs correlate replies, not deduplicate commands.

Messages are limited to 16 KiB, streamed turn output to 256 KiB, and the visible transcript to
32 messages with 64 KiB per reply. Outbound commands and inbound events use bounded queues; turns
have a ten-minute deadline. Connection state, drafts, messages and approval tokens are excluded
from serialized UI preferences and project files. Direct native edits persist normally when saved.

**Voice On** starts Codex's built-in real-time voice service in the same conversation; **Voice Off**
stops both the microphone connection and spoken playback. FilmCraft supplies only the native
WebRTC/CPAL audio adapter; Codex supplies the speech/conversation service. Voice uses the existing
Codex sign-in through WebRTC (the installed WebSocket audio transport requires separate API-key
authentication). The selected reasoning model is applied before starting voice, using a brief
setup turn when it differs from the conversation's current model. Model changes are disabled while
voice or another turn is active. No microphone or speakers are opened by Connect alone.

Microphone capture begins only after the voice service and media connection are ready. Closing
the console stops voice; ordinary text chat remains connected. Spoken edits use the same direct
native command/Undo path as typed chat. This initial audio profile is PCMU at 8 kHz (telephone bandwidth);
listening pauses while spoken output plays to prevent speaker feedback. A macOS `.app` bundle
with its microphone usage description is required: command-line launches fail with an actionable
error before touching the protected microphone API. The audio and event queues are bounded;
disconnect, device failures and stopped sessions close the owned streams and media peer.

`ui.inspect.console` reports `codex` (`disconnected`, `connecting`, `ready`, `failed`), `busy`,
`pendingEdit`, available/selected models, `voice`, sent/received voice frame counts, completed
FilmCraft inspection count, and the bounded session transcript in `messages`. Widget IDs:
`console.codex.toggle`, `console.codex.connect`, `console.codex.disconnect`, `console.chat.input`,
`console.chat.send`, `console.chat.cancel`, `console.chat.approve`, `console.chat.reject`,
`console.model`, `console.model.default`, `console.model.option.<model-id>`, `console.voice.on`,
`console.voice.off`. The same controls are commands: `console.model.set {model: id|null}`,
`console.chat.send {message: text, model?: id|null}`, `console.chat.cancel {}`,
`console.chat.approve {}`, `console.chat.reject {}`, `console.voice.on {}`, `console.voice.off {}`.
The assistant's console MCP profile cannot invoke these controls. Approve/Reject remain legacy explicit-preview controls, not the routine direct-command workflow.

Other provider adapters and standalone voice for agents without a built-in voice service remain future stages. The development checkout does not
update the installed app. Protocol reference:
[Codex App Server](https://learn.chatgpt.com/docs/app-server).

### Console integration qualification (2026-10-29)

The later `filmcraft-console/3` qualification at `18ac73c8` supersedes the earlier review-ticket-only
result. Local evidence is in `target/command-console-workflow-20261029/qualification.json`, its review
packet and `quality-checks.log`. The original project and everyday installation were unchanged.

Voice input selects one physical channel instead of averaging a multichannel interface.
`console.voice.channel.set {channel: 1}` chooses physical input 1 while voice is off and
no turn is running. The channel chooser has automation id `console.voice.channel`.
During voice, microphone and transmitted-level meters distinguish capture from echo gating;
`ui.inspect.console.voiceInput` exposes the device, channel count, peaks and gate state.
Small playback noise does not trigger the speech echo gate. The user confirmed Codex voice worked
in a later attended demo. A separate generated-speech test passed returned audio and same-thread MCP
inspection without opening devices; RTP counts alone do not establish microphone recognition.
The editing acceptance below kept voice off.

Native `console_analyze` requests music (6–600 seconds), performance matching or passage search
(2–600 seconds) on immutable prepared inputs in a guarded background worker. Only one analysis
runs at once; at most eight results are cached for ten minutes. Exact beats are returned in pages
of at most 128, with separate source-relative and timeline Tick positions. Section suggestions,
detected beats, audible onset and verified downbeats are distinct; the latter two are not claimed
by this detector. `console_read_analysis` rejects results after project revision, identity,
document epoch or active-sequence changes. Apply owned IDs directly with `command_run` and
`clip.analyzeMusic.apply` (including bounded section overrides), `clip.matchPerformance.apply`
(including bounded timing anchors), or `clip.trimPerformancePassage` (explicit phrase/boundary
review required). The app resolves its own plan/report and rejects model-supplied replacements.
Native validators and Undo govern mutation; similarity alone does not prove musical correspondence.

The later real-Codex GUI run passed media import, camera/studio sequence placement, nonzero
playhead movement and timeline selection, direct grouped edits with one native Undo/Redo,
full-song performance candidate analysis and Beat Cuts/section application (67 clips, 278 markers),
first-save memory/thread continuity, same-thread reconnect and actual Resume-picker use after app
restart. An eight-second 360×640/30 fps H.264/stereo AAC export completed; all 240 frames decoded.
The owned app closed and released its lease. At that checkpoint all 3,987 tests passed (49 skipped),
with strict lint, doctests, layers, assets and WebAssembly gates passing; later changes need fresh checks.

`fullDemoReady` remains false: live performance-match application and musical phrase/boundary/drift
acceptance were not verified; passage trim has a synthetic regression, not a director-reviewed
real-media trim. The export was eight seconds, not a full-song render, and not every registered
command was individually tested. Preserve ignored reports, project/media copies, companion memory,
application archive (`target/command-console-user-data/`) and Codex's own thread history before archival.

A streamed-response burst exposed a transient-full event queue disconnect. The bounded native
queue now waits for UI capacity in order, requests repaint, stops on disconnect, and reports a
30-second consumer stall instead of failing on an ordinary burst. A delayed-consumer regression
covers event order and shutdown. Proposal details scroll within a bounded region so large native
analysis plans leave the Apply/Reject controls reachable.
