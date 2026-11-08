# Control protocol & MCP

## Ordinary desktop launch

Without an explicit control port, the desktop application now creates its own authenticated
loopback connection for the embedded Console. It binds port zero once, retains that listener,
and injects a fresh random credential into the native provider adapters without changing the
process environment. No checkout, launcher script or pre-existing credential file is needed.
The private port and credential are not a public discovery interface. Connecting Desktop
publishes its existing owner-only MCP setup files for the external client. No AI provider or
microphone starts merely because the application opened.

The explicit developer interface below remains available for scripts and native acceptance tests.

## Desktop control channel
`filmcraft --control 9876` (or `FILMCRAFT_CONTROL_PORT`) listens on `127.0.0.1:<port>` (loopback only).
The listener requires `FILMCRAFT_CONTROL_TOKEN`: 64 random hex characters shared with authorized
clients through their inherited environment. Generate a fresh token for each desktop session, then
start the desktop and bridge from that environment:

```sh
export FILMCRAFT_CONTROL_TOKEN="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"
filmcraft --control 9876 &
filmcraft-cli mcp --bridge 127.0.0.1:9876
```

The CLI / MCP bridge reads this variable automatically. Keep it private: don't put the value in
command-line arguments, logs, checked-in MCP configuration, or shared files. Without a valid value,
the desktop leaves the listener closed. Every raw JSON request must include the same top-level
`token` string; missing or incorrect credentials get an error and close the connection before any
request is dispatched. Loopback alone does not restrict clients to the desktop owner's OS account.

Socket reads and writes time out after two minutes without progress, releasing the connection slot.
Expiration closes the connection without an unsolicited reply. The CLI/MCP bridge starts every
non-replayable request on a fresh connection, including edits, PDF ticket admission and consuming
ticket-status reads. Only explicitly retry-safe inspections reuse pooled connections and may retry
once on a fresh connection. Starting fresh is not replaying a transmitted request. Waiting for a UI
reply has its separate 60-second limit. A partial request that times out is not dispatched.

An absent or malformed reply does not prove a complete request was never dispatched. In particular,
do not treat a valid `{"ok":false,"error":"timeout"}` UI-reply response as cancellation: the original
queued edit/job can still execute later. The shared CLI/MCP bridge retries only an exact allowlist
of retry-safe reads. After a mutating request may have been transmitted, a partial write, lost reply,
read timeout or UI-reply timeout returns an actionable unknown-outcome error and discards the
connection; the client never silently repeats that mutation. Let the original request settle and
inspect project/history/jobs before deciding whether a new request is needed. The request `id` is
echoed for reply correlation, not a server-side deduplication key.

One JSON request per line → one JSON reply per line. The app opts out of macOS App Nap
(`apps/filmcraft/src/app_nap.rs`): a hidden window would otherwise drop the whole process to
background priority, and on a busy machine it would stop answering.

**Only requests are read.** Every line must be a JSON object with a string `method` (`id` and `params`
are optional; blank lines are skipped). Anything else gets one error reply
(`{"ok": false, "error": "… closing the connection"}`) and the server **closes the connection**, so
nothing sent after it on that connection runs. That covers text that isn't JSON, a JSON array or number,
an object without `method`, invalid UTF-8, and a line longer than 4 MiB. An HTTP request (for example a
web page's cross-origin `fetch` to `127.0.0.1:<port>`) therefore can't smuggle a command in its body:
its request line is rejected first. At most 16 connections are served at once; further ones get an error
line and are closed. Clients that get an error reply should reconnect. Transport:
`apps/filmcraft/src/control_server.rs`.

```json
{"id": 1, "token": "<FILMCRAFT_CONTROL_TOKEN>", "method": "engine.execute", "params": {"command": "sequence.addEdit", "params": {"seconds": 3}}}
{"id": 1, "ok": true, "result": {"cuts": 2}}
```

Methods (handlers in `crates/ui-egui/src/control.rs`):

| Method | Params | |
|---|---|---|
| `engine.execute` / `ui.menu.invoke` | `{command, params}` | any engine or UI command id |
| `engine.commands` / `ui.menu.list` | – | command registry / menu tree |
| `ui.inspect` | – | UI state (tool, workspace, dock, timeline view, playback, fps…) |
| `ui.docking` | – | the panel layout (craft-ui `Layout`), the focused and maximized panel, and `hitRects`: each docking gesture source drawn last frame (`{hit, rect}`) |
| `ui.elements` | `{prefix?}` | every on-screen interactive element: id, label, rect; `window` (the `n` of `panel.float.<n>`) marks an element drawn in a floating panel's own OS window, whose rect is in that window's coordinates |
| `ui.set` | `{tool, workspace, mode, theme, focused, playbackRes, timeline:{pps,scroll,fit}, program:{…}, source:{…}, clipDialog:{param: value}, menuDialog:{param: value}, panels:{scopes, timecode, reference, events, progress}, export:{…}, keyframeSelection:[…]}` | `keyframeSelection` replaces the Effect Controls keyframe selection (`[{clip, effect, param, mask?, time}]`, `time` in media ticks; read back as `ui.inspect` → `ui.keyframe_selection`; a click on a lane keyframe selects just that one); `program` / `source` merge fields into the monitor view state (`res`, `paused_res`, `high_quality`, `display`, `zoom`, `pan`, `show_rulers`, `show_guides`, `lock_guides`, `snap`, `guides`, `compare_ref`, …; see [monitors.md](monitors.md)); `clipDialog` sets fields of the open Edit / Clip / File dialog (Paste Attributes, Make Subclip, Frame Hold Options, …); `menuDialog` those of the M3.11 dialogs (Find, Create Search Bin, Project Settings, Scene Edit Detection, Simplify Sequence, Automate to Sequence…; see `panels::menu_dialogs`); `panels` merges into the panel settings (`ui.inspect` → `ui.panels`; see `panels::panel_state`): Lumetri Scopes `{shown: ["vectorscopeYuv"|"vectorscopeHls"|"histogram"|"parade"|"waveform"], waveformType, paradeType, colorSpace, brightness, scale, clamp, targets}`, Timecode `{rows: [{source, mode, display}], showName}`, Reference Monitor `{ganged, time, display: "composite"|"scopes", scopes}`, Events `{level, selected}`, Progress `{showFinished}` `export` deep-merges into Export mode state (`preset` — applied first —, `settings` (ExportSettings JSON), `fileName`, `location`, `range`, `customStart`, `customEnd`, `openSections`, `manager` (Preset Manager: `query`, `favoritesOnly`, `selected`, `saveName`), `quickOpen`, `quickPath`, `quickPreset`); `program` / `source` merge fields into the monitor view state (`res`, `paused_res`, `high_quality`, `display`, `zoom`, `pan`, `show_rulers`, `show_guides`, `lock_guides`, `snap`, `guides`, `compare_ref`, …; see [monitors.md](monitors.md)); `clipDialog` sets fields of the open Edit / Clip / File dialog (Paste Attributes, Make Subclip, Frame Hold Options, …); `menuDialog` those of the M3.11 dialogs (Find, Create Search Bin, Project Settings, Scene Edit Detection, Simplify Sequence, Automate to Sequence…; see `panels::menu_dialogs`); `projectPanel` merges into the Project panel's view state (`bin` shown in place, `tabs` (bins in a tab or window: `{bin, floating, view, iconSize, nav}`), `activeTab`, `selectedBin`, `rename: {item|bin, text}`, `dialog` (`{"metadataDisplay": {columns, filter}}`, `{"savePresetAs": {name}}`, `{"managePresets": {selected, name}}`, `{"freeformOptions": {options}}`, `{"saveArrangement": {name, bin}}`)); `mediaBrowser` into the Media Browser's (`expanded`, `pathEdit`, `editColumns`, `treeWidth`). View settings, columns and presets are engine commands (`project.view.set`, `project.columns.set`, `project.viewPreset.*`, `mediaBrowser.settings`) |
| `ui.panel.show` / `ui.panel.close` | `{panel}` | |
| `ui.click` / `ui.move` | `{id}` or `{x,y}`, `button`, `count`, `modifiers` | synthetic pointer input; click `count` is an integer 0–10 (default 1); `ui.move` ignores it |
| `ui.drag` | `{from, to, steps, modifiers}` | press–move–release; `steps` is an integer 0–1024 (default 12; 0 or 1 uses 2 steps) |
| `ui.scroll` | `{id|x,y, dx, dy, modifiers}` | wheel / trackpad |
| `ui.key` / `ui.type` | `{key}` (`Cmd+K`, `Space`…; off macOS `Ctrl+K` is the same key) / `{text}` | keyboard |
| `ui.timeline.hit` / `ui.timeline.locate` | `{x,y,modifiers?}` / `{clip, edge?}` | timeline hit-testing. `hit` reports a clip's `edge` (`In`/`Out`) within 7 px of it (a third of a narrow clip); just left of a cut that is the left clip's Out, from the cut rightwards the right clip's In. `kind` is the trim a press of the current tool with `modifiers` starts there (`trim`, `ripple`, `roll`, `rateStretch`, `remix`), `volume` on an audio clip's Volume line (Selection and Pen tools), or null. On the ruler it reports `ruler`, or `workArea` with the bar's `edge` on the work area bar |
| `ui.playback` | `{action: play|stop|toggle, speed?}` | |
| `ui.screenshot` | `{path?, panel?}` | PNG of the window or one panel; fails after 10 s when no frame is presented (window hidden, display asleep) |
| `ui.resize`, `app.quit` | `{width,height}` / `{}` | resize or close the native window; `app.quit` quits without the save prompt (unsaved changes stay in the recovery journal), while the command `app.quit` asks first |
| `ui.focus` | `{}` / `{id}` | empty parameters activate the native window; a widget ID requests egui focus without OS activation |
| `perf.stats` | – | performance counters (also the command `perf.stats`, so `engine.execute` and MCP `command_run` reach it; headless sessions return the engine part). `decode`: GOP-cache requests / hits / `cacheHitRate`, decoder seeks, samples decoded and skipped while catching up, `draftFrames` (decoded in draft mode), `h264Threads` (frame-threading workers per H.264 decoder), `liveDecoders` (sources holding a decoder now), `cacheMB` / `cacheBudgetMB` (decoded frames all sources hold, and the shared budget), `planePoolMB` / `planesReused` (idle buffers of the frame pool, the planes of evicted frames and the compositor's float images, which a standalone export frees when it ends; and buffers taken from it instead of allocated), decoder ms (total and per sample), `framesDecoded`, `hardware` (Settings ▸ Playback ▸ Hardware decoding: `enabled`, pictures from hardware decoders `frames` vs `softwareFrames`, `sessions`, `declined` streams, mid-stream `fallbacks`); `playback`: playing, shown / dropped frames and `dropRate` of the current or last play, resolution, `draftDecode` (Settings ▸ Playback ▸ Draft decoding), `audio` (desktop: sound is mixed 200 ms ahead of the device; device `callbacks`, `underruns` and `missingFrames` played as silence because the mixer fell behind, `minLeadMs` the least sound left buffered); `frames`: frame-worker jobs, cancelled, `requestHitRate`, `decodeMs` / `renderMs` (mean, p50, p95 of the last 256 jobs), queue, render-cost estimate, cache use; `monitor`: the `frame` and `revision` of the picture the Program monitor shows against `playheadFrame` and `projectRevision` (how far a drag or a scrub is ahead of the picture); `ui`: fps and frame ms; `process`: CPU seconds; `media`, `jobs`. Counters are cumulative: diff two readings to measure an interval ([performance.md](performance.md)) |

For `ui.click` and `ui.drag`, omitted or null counts/steps use the defaults. Out-of-range or
non-integer values return `{"ok": false, "error": "…"}` before queuing any pointer input.
These limits also apply to the MCP bridge's `ui_click`, `ui_drag` and `ui_control` tools.

**Timeline thumbnails.** `ui.set {timeline: {thumbnails}}` sets the Timeline wrench menu's Show Video
Thumbnails: `off`, `head`, `headAndTail` or `continuous`.

**Selection or explicit targets.** A command that acts on the selection is "not available right now" when
nothing is selected. When `params` name the targets under a key the command documents (`clips` /
`clip`: timeline clips of the active sequence, `items` / `item`: project items), enablement is
checked against those instead and the selection is left as it is; the command's other conditions
still apply. `engine.commands` and the menus always show the selection's enablement.

**Focus.** Driving the app never steals the user's keyboard. Started with `--control`, the app opens
without activating itself. UI requests and screenshots that need a rendered frame bring the window
forward without making it key (macOS `orderFrontRegardless`), so the user's typing keeps going to
the app they are in. Only an explicit `ui.focus {}` activates FilmCraft. Injected keys work on
focused widgets even when the native window is in the background.

`ui.focus {id}` targets an enabled, visible widget registered with a real egui response. It rejects
unknown IDs, non-widget regions and hidden tool aliases. The reply acknowledges the request;
`ui.inspect.keyboardFocus` reports the actual widget ID, matching element IDs, label and active
focus. This is separate from `ui.focused`, which identifies the active panel. Tools, dock tabs,
tab menus and divider handles expose widget IDs through `ui.elements`. Tab/Shift-Tab move widget
focus; Space/Enter activate focused buttons and tabs, with visible focus outlines.

For unattended validation, start with `--control <port> --control-input-only`. This opt-in mode
accepts widget input through control requests and excludes native keyboard, pointer, file-drop
and menu commands. Window close/quit and screenshots remain available. Ordinary launches and
`--control` alone keep native input enabled. `ui.inspect.input` distinguishes `mode`, the observed
`nativeWindowFocused`, and egui's `effectiveWindowFocused`; effective focus does not activate the
OS window or prove physical-keyboard delivery.

Element ids are stable, e.g. `timeline.clip.<id>`, `timeline.track.V1.lock`, `tools.Razor`,
`project.item.<id>`, `effects.item.gaussian_blur`, `panel.tab.Timeline`, `program.transport.playback.toggle`.

Audio mixing: `mixer.*` commands (strips `"A1"`, `"S1"`, `"Mix"` or ids; lanes `volume`, `pan`,
`mute`, `send.<i>.level`, `fx.<slot>.<param>`): `mixer.inspect`, `mixer.setStrip` (volume, pan, mute,
solo, record arm, solo safe, mode Off/Read/Latch/Touch/Write, output, input map, channels),
`mixer.setValue`, `mixer.touch` / `mixer.release` (a fader gesture; recorded during an automation pass),
`mixer.recordStart` / `mixer.recordStop` (playback runs them), `mixer.addSubmix`, `mixer.deleteSubmix`,
`mixer.addInsert` / `removeInsert` / `setInsert`, `mixer.addSend` / `setSend` / `removeSend`,
`mixer.setKeyframe` / `deleteKeyframe` / `moveKeyframe` / `clearLane`, `mixer.writeAutomation`;
`clipMixer.set`; `clip.audioGain {clips?, mode: set|adjust|normalizeMax|normalizeAll, db}`, `clip.audioPeak`;
`effects.setDefaultTransition`. UI ids: `mixer.<A1|S1|Mix>.<fader|value|pan|panValue|mode|mute|solo|
record|soloSafe|output|input|fx.<n>|send.<n>|meter|name>` (popup entries below them, e.g.
`mixer.A1.mode.Touch`, `mixer.A1.fx.0.studio_reverb`), `mixer.showEffects`, `mixer.transport.<cmd>`,
`clipMixer.A1.<fader|pan|mute|solo|keyframe|value>`, `timeline.track.A1.keyframes[.<lane>|.clip]`,
`timeline.track.A1.lane[.kf.<n>]`, `audioGain.<set|adjust|normalizeMax|normalizeAll|ok|cancel|peak>`.

Essential Sound: `essentialSound.inspect`, `essentialSound.setType {type: dialogue|music|sfx|ambience}`,
`essentialSound.clearType`, `essentialSound.set {key, value}` or `{values: {key: value}}` (keys such as
`repair.noise.on`, `repair.noise.amount` (0–10), `repair.humHz`, `clarity.eqPreset`, `clarity.enhanceTone`,
`creative.reverbPreset`, `ducking.against`, `ducking.reduceDb`, `ducking.fadeS`, `pan.value`,
`volume.levelDb`, `mute`, section switches `<section>.enabled`), `essentialSound.applyPreset {preset, type?}`,
`essentialSound.savePreset {name}`, `essentialSound.deletePreset {name}`, `essentialSound.autoMatch {target?}`,
`essentialSound.generateDucking`; all take `clips` (default: the selected audio clips). UI ids:
`essentialSound.tab.<Browse|Edit>`, `essentialSound.type.<Dialogue|Music|SFX|Ambience>`,
`essentialSound.clearType`, `essentialSound.preset[.save|.delete|.name|.ok]`,
`essentialSound.section.<Name>[.toggle]`, a row per setting key (`essentialSound.repair.noise.on`,
`essentialSound.repair.noise.amount`, `essentialSound.repair.humHz.<i>`), `essentialSound.autoMatch`,
`essentialSound.ducking.against.<Type>`, `essentialSound.generateDucking`, `essentialSound.volume.on`,
`essentialSound.volume.levelDb`, `essentialSound.mute`, `essentialSound.browse.<Type>.<preset>`.

Project files, auto-save, crash recovery and preferences commands (`file.recover`, `prefs.set`, …) and their
automation ids are listed in [project-files.md](project-files.md). That file also lists the media
management commands and dialog ids: offline media and relinking (`media.findMissing`,
`media.relink`, `media.autoRelink`, `media.search`, `media.makeOffline`, `media.status`, `linkMedia.*`),
proxies (`media.createProxies`, `media.attachProxies`, `media.toggleProxies`, `proxies.*`), ingest
(`project.ingestSettings`) and the Project Manager (`file.projectManager`, `pm.*`).

**Moving clips**: `timeline.move {moves: [{clip, track, time}], insert?, linked?}` → `{moved: [clip ids]}`.
With Linked Selection on (the default; `linked` overrides the toggle for one call) the linked
partners of a listed clip that are not listed themselves move by the same time offset and stay on
their own tracks, so picture and sound stay in sync. Like a listed clip, a partner overwrites
what is at its destination (or inserts, with `insert`). A partner on a locked track stays where it
is, and so does one whose position would not change (the listed clip only changed track). `moved`
lists every clip that moved, partners included. No clip is moved before the sequence
start: if one would be, all clips of the call land later by the same amount (their spacing is
kept). A `time` beyond the representable range is an "invalid parameters" error. The timeline
panel's drag passes `linked: false`, because it already lists exactly the clips it moves. With
`copy: true` (Option-drag in the timeline) the clips stay where they are and copies land at the
destinations instead (linked partners are copied too, unless `linked: false`); the result lists
the new clips as `copied`, they are selected, and copies of linked clips are linked to each other.

**Trim mode** (`crates/engine/src/trim.rs`): `trim.selectEditPoint` / `trim.selectNearest` enter trim
mode (the Program monitor becomes the Trim Monitor, ids `trimMonitor.*`); `trim.monitor` returns what
it shows. With Linked Selection on, selecting an edge includes its unlocked linked partners, even
on untargeted tracks; `add: true` (Shift-click) toggles the group. Regular/ripple trims select the
corresponding In/Out edges. Rolls include only matching linked cuts at the same time, leaving split
audio cuts separate. Each linked group moves once and shares its tightest trim limit; turning
Linked Selection off allows individual edges to be selected and edited.
Dynamic trimming takes an explicit clock (seconds) so it is deterministic: `trim.shuttle
{direction, slow?, clock}` (J/L), `trim.tick {clock}`, `trim.shuttleStop {clock?}` (K, one undo step),
`trim.cancelDynamic` (Esc), `trim.playAround {clock, loop?}` (Space / Shift+K).

**Replace With Clip**: `clip.replaceFromSource`, `clip.replaceFromSourceMatchFrame` and
`clip.replaceFromBin {clips?, item?, keepSourceIn?}` swap the media of the clips and return
`{clips: [ids], item, short: [{clip, shortByFrames}]}`. `short` lists the replaced clips that now
run past the end of their media (the renderer holds the last frame there) with the number of
sequence frames that have no media, at most the clip's length; it is empty when the new media
covers every clip. The edit is the same either way. From Bin starts each clip at the new item's In point; with
`keepSourceIn: true` each clip keeps its own source In instead. A subclip that restricts trims has
no media outside its range, so there a kept In is moved to the subclip's first or last frame, and
the result's `sourceInClamped: [ids]` (present with `keepSourceIn`) names the clips it was moved for.

**Colour** (`crates/engine/src/color.rs`): `sequence.colorSettings {workingSpace: rec709|rec2100-pq|
rec2100-hlg, wideGamut, autoToneMap}`, `clip.interpretFootage {items?, colorSpace?: auto|<id>, pixelAspect?: [num, den]|file}`,
`color.spaces`, `media.colorInfo {item}`; LUTs: `lut.import {path, name?}`, `lut.list`,
`lut.remove {id}`, `lut.export {lut, path, format?}`, `lumetri.setInputLut` / `lumetri.setLook
{clip?, lut: lib:<id>|builtin:<id>|"", path?}`, `lumetri.setSection {clip?, section, on?}`,
`lumetri.applyMatch {clip?, referenceTime|referenceFrame|referenceTimecode, faceDetection?}`. Without
params the menu entries open dialogs (ids `colorDialog.space.<id>`, `colorDialog.working.<id>`,
`colorDialog.wideGamut`, `colorDialog.autoToneMap`, `colorDialog.ok|cancel`). Lumetri panel ids:
`lumetri.input_lut`, `lumetri.look_lut`, `lumetri.switch.<section>`, `lumetri.section.<section>`,
`lumetri.match.*`; HDR scopes: `scopes.hdrWaveform`. `file.exportMedia {sdr: true}` exports an HDR
sequence as tone-mapped SDR.

**Keyboard shortcuts** (`crates/engine/src/shortcuts.rs`): `shortcuts.list {query?, panel?}`,
`shortcuts.get`, `shortcuts.set {command, keys, panel?, add?, keepConflicts?}`, `shortcuts.clear`,
`shortcuts.undo` / `shortcuts.redo`, `shortcuts.conflicts {platform?}`, `shortcuts.forKey {key}`,
`shortcuts.resolve {keys, panel?}`, `shortcuts.presets`, `shortcuts.loadPreset` / `savePreset` /
`deletePreset {name}`, `shortcuts.export` / `import {path}`, `shortcuts.audit`. Keys use `Cmd` (⌘ /
Ctrl), `Ctrl` (macOS ⌃), `Alt`, `Shift`. The dialog (Edit ▸ Keyboard Shortcuts…, ⌥⌘K) uses ids
`shortcuts.*` (`shortcuts.key.K`, `shortcuts.cell.<command>`, `shortcuts.ok`, …).

## MCP
`filmcraft-cli mcp` serves MCP on stdio: headless (in-process session; `--demo` / `--project p.fcproj`)
or `--bridge 127.0.0.1:9876` to drive the running app. Tools: `command_list`, `command_run`,
`command_batch`, `doc_inspect`, `render_preview`, `project_inspect`, `sequence_inspect`, `media_import`,
`render_frame`, `ui_inspect`, `ui_elements`, `ui_click`, `ui_drag`, `ui_key`, `ui_type`, `ui_screenshot`,
`ui_control`; resources `filmcraft://document` and `filmcraft://commands`. `.mcp.json` registers both.
Annotations, argument checking, errors and export progress / cancellation:
[agents.md § Conventions](agents.md#conventions).

Time is in ticks (254 016 000 000 per second); commands also accept `seconds`, `frame` or `timecode`.

## Panel docking commands

`ui.menu.invoke` and `engine.execute` route these frontend commands:

| Command | Parameters | Result |
| --- | --- | --- |
| `window.floatPanel` | `{panel?, rect?: [x,y,width,height]}` | Move an existing panel into a floating group. Omitted panel uses the focused panel; without a rect it floats inside the dock area, 24 points from its top-left corner, at the size a drag floats it at. |
| `window.dockPanel` | `{panel?, anchor?, zone?: "center"\|"left"\|"right"\|"top"\|"bottom", before?}` | Move into the anchor's tab group or a side split; `before` names an existing destination tab for precise ordering. Floating destinations accept tabs only. Without `anchor`, a side `zone` docks the panel along that edge of the whole workspace, at a quarter of its width or height. |
| `window.closePanel` | `{panel?}` | Close a docked or floating panel without closing its project documents. |

Invalid source/target identities and non-finite or out-of-range geometry return an error with
the layout unchanged. `ui.inspect.ui.dock.floating` reports floating windows and their
logical-point rectangles. `panel.drag.<Panel>` locates the drag bar of each panel's group;
`panel.float.<index>.move`, `.resize` (bottom-right corner) and `.resize.<Edge>` locate floating
geometry handles; `dock.gutter.<path>`
locates a split divider (`a`/`b` per split level), `dock.corner.<outer path>.<inner path>` the
four-way handle where two dividers meet, `panel.float.<index>.gutter.<path>` and
`panel.float.<index>.corner.<outer path>.<inner path>` the same inside a floating window;
`ui.docking` lists every docking gesture source. Named workspace saves include floating windows.

The panels use craft-ui's shared docking with its split-tree settings. Its control commands
(`window.panel.move`, `.float`, `.dock`, `.activate`, `.close`, `.closeGroup`, `.closeOthers`,
`.minimize`, `.layout`; see craft-libs `docs/ui-docking.md`) route here too; the three
commands above are FilmCraft's names for `window.panel.float`, `.dock` and `.close`.

`window.dockAction` accepts `{action}` using the shared docking action's JSON form. It
covers tab moves and ordering, split and floating geometry, activation, and closing.
For example, `{"action":{"ResizeSplit":{"path":[],"size":{"Ratio":0.3}}}}`
resizes the root split; path entries use `false` for the first child and `true` for
the second. `{"action":{"MoveFloating":{"panel":"Info","rect":[45,65,420,320]}}}`
repositions and resizes the floating group containing Info. Invalid actions leave the
layout unchanged. Pointer gestures dispatch through this same catalog command.

### Preserved custom controls and diagnostics

`playhead.returnToZero {}` seeks exactly to zero (Timeline's default Return binding).
`tool.marquee {}` selects the dedicated selection-only box tool. Its Timeline-scoped default is
Control+Command+M on macOS and Shift+V on Windows/Linux; M adds a marker in every panel by default.
`tools.shortcuts` opens the shortcut editor through the Tools panel.
`diagnostics.toggle {}` toggles opt-in local Flight Recorder monitoring.
`diagnostics.capture {}` opens its capture dialog; `diagnostics.capture {seconds: 1..600}` writes a
local report and returns its path, or an actionable error. Reports omit command parameters and
media paths. `prefs.set {key:"general.persistentUndo",value:true}` enables optional bounded Undo
sidecars for subsequent saves and opens. It does not import unmatched legacy history.

### Bounded controller profiles

`filmcraft-cli mcp --controller-read-only` exposes inspection, supported-request planning and a
current-frame preview. `--controller-edit` adds one-use preview/commit tools for the preserved
bounded editing operations. Both support `--bridge` with the existing control token. They are
mutually exclusive. The default MCP server remains available without either flag.

`controller.capabilities {}` reports guarded operations supported by this app. Controller commits
include `expectedRevision` and `sequenceId`; malformed or stale guards fail before editing. Marker
commits use `target:"program"` explicitly. Tokens expire after 300 seconds and are not reusable.
See `crates/automation/README.md` for scopes, approval semantics and remaining voice limitations.

### Coarse performance passage search

`clip.findPerformancePassages {video: clipId, song: clipId}` performs read-only
coarse retrieval in the selected clips' current source ranges. It returns up to
five separated candidate intervals, relative to the live clip's source In, plus
full `video`/`song` snapshots and the 100 ms feature resolution. `rankingScore` is an uncalibrated
ranking value, never a probability. `requiresReview` is always true;
`arrangementVerified` and `boundariesVerified` are always false. Sustained tails
and repeated or missing sections can give misleading boundaries. The response is
not an apply-compatible match plan and does not trim, retime or select a take.
Phrase confirmation and fine event timing are still required before matching.

`clip.trimPerformancePassage {report, start: ticks, end: ticks,
phrasesConfirmed: [true,true,true], boundariesReviewed: true}` explicitly trims
the reviewed live range and identically edited linked audio in one Undo step.
The user must confirm corresponding start/middle/end context and the complete
arrangement and exact bounds. Stale snapshots, locks, touching transitions and
differently edited linked audio refuse the edit. This command does not retime;
run timing analysis only after trimming. The dialog's Source-preview and
capture-In/Out controls avoid manual timestamp transcription. Confirmation is
user evidence, not a measured synchronization guarantee or musical signoff.


### Bounded Console inspection (contract v5)

`project.inspectPage` and `sequence.inspectPage` are read-only engine queries available through
`engine.execute` and CLI/MCP. Both accept `offset`, `limit` (1–64, default 32), and
`expectedRevision`; offsets after zero require the revision returned by the first page. Results
include `revision`, `rows`, `total`, `nextOffset` and a 24 KiB budget. Edits invalidate a paging
revision. Sequence pages accept `item` (sequence ID), `track`, `clip`, and `kind`: clips (default),
tracks, markers, selection, effects, parameters or keyframes. Effects need a clip ID, parameters
also need `effect` (zero-based index), and keyframes also need `param` (name). Selection IDs are
paged separately. These queries do not edit the project or history and do not recursively expand
nested sequences. Names and paths are capped at 512 UTF-8 bytes; oversized parameter values are
reported as omitted rather than silently truncated into a plausible value.

The in-app Console maps its project/sequence tools and legacy full inspection requests onto
these pages. Its compact interface query omits chat transcripts and widget trees. The general
native inspection protocol keeps its existing full-result behavior. See [agent usage](agents.md).

### Inline AI captures

`ui.screenshot.inline {panel?, id?, maxSide?, range?}` returns an in-memory PNG (`pngBase64`), not a file. `maxSide` must be an integer in 64–4096 (default 1600). `inline`, if present, must be true; `path` is rejected. The dedicated method fails on older builds instead of silently creating a default screenshot file. `ui.screenshot` also accepts `inline:true`; explicit file output remains available and uses atomic replacement.

`range:[start,end]` is optional for an inline Timeline panel capture only: exact integer ticks with `0 <= start < end`, wholly inside the currently visible time range and at least one screen point wide. Offscreen ranges return an actionable error; capture does not pan or zoom the view. Timeline metadata includes visible/captured time ranges, zoom, waveform visibility, bounded visible audio/video track and clip identities, and truncation indicators. Pixel-derived timing is approximate. A changed view/document/revision rejects an in-flight capture.

PNG output is adaptively reduced to at most 256 KiB. Actual and original dimensions, project revision, active sequence and document epoch accompany it. FilmCraft retains no screenshot files for AI delivery; explicitly saved screenshots are independent. AI conversation history may retain delivered image content under the provider's normal behavior.

### Bounded command diagnostics

`ui.inspect.requestDiagnostics` exposes at most 32 in-memory command records and the last
observed revision-changing request. Compact Console `ui_inspect` returns the newest eight
records plus that last change. Each record contains a local numeric request ID, UTC start,
known registered command IDs (at most 16 per batch, with an omitted count), monotonic elapsed
time, before/after revision and document epoch, returned outcome and optional numeric job ID.
Unknown IDs become `<unregistered>`; labels, parameters, prompts, paths and result/error payloads
are never copied into this history. The in-memory view clears when the app exits; the same
fixed admission/return metadata is written to the existing capped application log.

`pending` means outcome unknown. `returned_ok`/`returned_error` describe the app dispatcher,
not response delivery or asynchronous job completion. A revision change or last command is a
correlation clue, never proof of crash/hang cause; inspect before retrying an uncertain edit.
This history covers control command dispatch, not every UI gesture or screenshot lifecycle.
Connection initialization, config, session start/resume and MCP discovery emit fixed metadata
breadcrumbs to the existing capped application log (16 MiB, two previous logs retained).
Raw protocol frames, thread IDs and configuration payloads are not added to those breadcrumbs.


`ui.inspect.audioOutputNote` is a cached native-output warning or null. Reading it performs no
hardware query and is not a readiness guarantee when null. A native device call that exceeds its
worker deadline can report unresponsiveness while editor control remains available.


### Selected reference image snapshots (Console contract 7; in validation)

`console.attachments.inspect` includes `documentEpoch` and each attachment's stable `snapshotId`.
`console.attachments.image` takes exactly `{ "snapshotId": integer, "documentEpoch": integer }`
and returns bounded `pngBase64`, delivery/original dimensions and coverage for a selected PNG.
It reads the admitted snapshot, never reopens a source path, and is available during voice.
Removal, Send and project changes invalidate access. Non-image references and stale identities
return errors without changing the project, history, voice state or attachment selection.

The Console MCP `reference_image` tool takes `snapshot_id` and `document_epoch` and returns actual
ImageContent plus metadata. It has the image response budget; ordinary reference results retain
their smaller text budget. PNG delivery may resize the snapshot to fit 256 KiB and retains alpha.
No generated image file is written. Native/package acceptance is still pending.


### Indexed PDF references (Console contract 8; in validation)

On the native FilmCraft host, Add Files/drop admits a PDF asynchronously. A selected attachment
exposes `pdf.status` (`loading`, `ready`, `failed`), `pdf.pages`, its stable `snapshotId` and
`documentEpoch`. Failure remains visible beside the selected reference. Send waits for selected
PDFs to become ready; loaded PDFs remain selected for subsequent page reads until Remove or
project switch. First-page excerpts are replaced by compact metadata after Send. Ordinary text
and PNG attachments keep their one-shot Send lifetime. User source files are never changed.

`console.attachments.pdf.read` takes `snapshotId`, `documentEpoch`, one-based `page`, optional
UTF-8 byte `offset` (default 0), and `limit` (1–4096 bytes, default 4096). It returns a pending ticket.
`console.attachments.pdf.status` takes the same identity/epoch and `ticket`; pending results remain
pending, while a completed result is consumed and includes page text and `nextOffset`. Removed,
stale-project and already consumed tickets return errors. Completed chunks and pending jobs are
bounded; reading a reference never changes the project or Undo state.

The read-only MCP `reference_pdf` tool takes snake-case `snapshot_id`, `document_epoch`, `page`,
`offset`, `limit`, or a pending `ticket` without page/offset/limit. It polls the existing ticket with
a wall-time budget; a pending result must be resumed rather than enqueueing the same page again.
No model-supplied file path is accepted. Extracted text grants no permissions. OCR, PDF pictures
and layout interpretation are not provided. The native helper supports up to 32 MiB/4096 pages;
`console.attachments.inspect` reports actual host support and limits. Other embedders retain their
existing small-PDF path unless they supply the supervised host hook.

### Isolated acceptance session policy

The desktop host accepts `FILMCRAFT_SESSION_LOCK_FILE` as an explicit launch-only
session-lock file override for disposable acceptance profiles. An unset or empty
value retains the normal user-level lock shared across builds. `--data-dir` alone
still does not redirect that policy. The override is not an MCP or control-channel
parameter; keep it unset for the installed owner app. Existing safe regular-file,
size, identity and atomic-write checks still apply to the selected policy file.
