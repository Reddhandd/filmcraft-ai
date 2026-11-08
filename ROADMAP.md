# FilmCraft Roadmap

Current owner scope: finish alpha stability and the requested Console workflows. Premiere
project import and broader portability are on hold until the owner explicitly confirms a change.

- **Console Studio layout delivered and native-qualified (2026-11-02).** The signed
  `58b92a2f` demo passed native appearance, hover-function and keyboard-focus label checks,
  settings disclosure and API-disablement checks. It opens with the existing memory profile and
  session lock preserved. Output precedes the composer, session actions share a toolbar, memory
  sits beside Save Assistant, and settings/advanced commands start collapsed. A reported physical
  Return no-op later passed the owner's healthy idle-state retest; its original transient cause remains unconfirmed.
- **Voice renewal transports successfully; conversational recall failed (2026-11-03).**
  `3b9412b2` passed the source-stable gate (4,394 passed / 51 skipped / 49 WASM crates) and
  packaged warning/55-minute renewal kept the same thread ID while cancelling unfinished voice
  work. The owner reported that the replacement voice did not remember the preceding conversation.
  Thread identity alone is not continuity acceptance. A short Voice Off/On or Codex reconnect
  suffices for the next recall test; another hour-long wait is unnecessary for this defect.
  The repair is locally committed as `245ef2cc`: bounded role-bearing startup history and assistant
  identity preserve no-replay rules and separate new conversations/projects. Generated-speech
  WebRTC testing reproduced lost recall with empty startup history, then retained the phrase and
  exact editing role with the handoff, without physical audio devices. The final source-stable
  gate passed 4,400 tests (52 skipped) and 49 WASM crates. A final generated-speech retest also
  passed with Finder-like PATH. The matching signed replacement passed a headless generated-project
  save-copy check and preserves the existing memory/settings profile. Short packaged Voice Off/On
  and full Console reconnect recall/role/no-replay acceptance are still required.

- **Active Codex turn deadline defect reproduced (2026-11-04).** Owner recurrence in the
  signed reconnect demo had working voice but no further editing, with repeated
  `responseStreamDisconnected` notices. The native project-context guard was clear. Separately,
  a working turn started at 00:13:40.397 UTC and FilmCraft terminated its worker at
  00:23:40.402 UTC; native edits were still succeeding at 00:23:34.721 UTC. The absolute
  ten-minute deadline therefore killed an active task. The repair candidate watches real task
  progress instead, cancels a genuinely silent task once, and waits up to 30 seconds for a real
  terminal acknowledgement before reporting failed recovery. Voice traffic, unrelated turns and
  upstream retries do not refresh task liveness; no old edit is replayed. Structured upstream HTTP
  status is retained when supplied, without logging headers or arbitrary error detail payloads.
  This does not establish why the upstream stream retries after reconnect; that failure and the
  owner's partial-detail recall remain open. Candidate qualification is in progress, and the
  running owner demo/project is unchanged. The repair subsequently passed its source-stable
  gate (4,405 tests passed, 52 skipped; 49 WASM crates), and the matching signed C36 demo
  was delivered. The owner still reproduced the reconnect editing failure in that build;
  the inactivity repair is not evidence that reconnect recovery is resolved.

- **Persistent Reference Library implementation (owner requested 2026-11-05).** Eight
  owner-selected filmmaking/editing/animation books are privately copied and indexed on D:
  1,665 physical pages, 2,367 passages, with matching source/copy SHA-256 hashes. A source
  implementation adds background import, local term search, bounded cited retrieval through the
  existing Console reference tool, shared enable/remove controls and recovery with a permanent
  removal ledger. Four books required a local page-preserving extraction using installed
  Poppler; it is not bundled or a product dependency. The running demo is unchanged until the
  matching build is installed. Commit `097dda3b` passed the full gate (4,410 tests,
  52 skipped; 49 WASM crates); the matching signed demo passed its headless save-copy
  check and preserved memory/library data during packaging. Twelve sampled physical-page
  citations passed an independent cross-check. Native controls and live AI retrieval remain
  unverified. A further source candidate adds project/assistant-specific selections and
  scoped retrieval; its full gate and packaged/native checks remain pending. No
  full-book upload, model training or OCR is claimed. See [Reference Library](docs/reference-library.md).

- **Direct API connections: disabled / unresolved (owner decision, 2026-11-01).** Future builds
  offer Codex and Desktop ChatGPT. The preserved API implementation is not an active alpha task.
  Live marker editing and native Undo passed, but the three-frame capture workflow failed and
  remains unresolved. No further API integration or paid testing is scheduled. See
  [API connection status](docs/api-connection-status.md).

- **Project Assistants are qualified in the signed `834634e9` build:** changed-context
  checkpoints every 15 minutes, milestone/close saves, named reusable assistants, project isolation,
  cross-build restoration and no edit replay passed their recorded checks. Matching native tests also
  verified automatic recovery after a persistence lock is released, previous-checkpoint recovery with
  a visible warning, and actionable failure when both checkpoints are invalid. The source-stable
  gate passed 4,358 tests (50 skipped) and 49 WASM crates. Evidence:
  `target/project-assistants-wakeup-native-20261101/native-acceptance-result.json`.

- **Named assistant autosave requested (2026-11-04).** The owner reports that a fresh
  session immediately loaded the assistant/controller instructions and understood the current
  task, although the saved information was slightly behind. Add an explicit Autosave option
  beside the selected named assistant. Existing 15-minute project checkpoints do not automatically
  replace the reusable named assistant. Autosave must show its last successful save and failures,
  preserve a previous version, and keep project-specific history isolated rather than copying it
  into other projects through the reusable assistant. Qualify updated assistant restoration in a
  fresh session without replaying edits. A [source candidate](docs/project-assistant-autosave.md)
  now implements the opt-in, bounded background updates and visible status. Full-gate and
  matching packaged/native qualification are pending; existing-session reconnect/edit recovery
  remains open.

  The manual save action now uses **Update Assistant** for an existing name in the source
  candidate, retaining that catalog entry's name and updating its current instructions.
  A new name still creates a separate assistant; stale-write checks and project-history
  isolation remain in place. This UI change is included in the signed `097dda3b` demo;
  native review remains pending.

- **New owner timeout reproduction:** after voice times out, cancelling the unfinished task
  beside Send permits voice reconnect. A source candidate tracks voice-originated turns and uses
  the existing interrupt protocol once when voice fails; it waits for actual task completion and
  leaves ordinary text tasks alone. Targeted/controller regressions and the source-stable
  `0978bfe1` full gate passed. Matching packaged active-task timeout/reconnect acceptance remains pending.

- **Context-refresh edit block:** owner logs show successful edits through revision134, then
  memory operations followed by a blocked playhead command while inspection remained available.
  A regression reproduces self-authored assistant memory unnecessarily requesting a context refresh
  that waits for voice to end. The source repair removes that invalidation without clearing genuine
  project-change guards. The after-fix full gate and matching private native idle memory-update/edit
  check passed. Matching `58b92a2f` native acceptance also passed an assistant checkpoint during
  active voice followed by an AI-requested marker edit in the same conversation, with no context
  block. This qualifies that memory-refresh defect; it does not prove unrelated unfinished-task
  reconnect cases. Evidence: `target/console-polish-demo-20261101/active-voice-context-native-result.json`.

- **Current implementation and verification:** in-app masked API key/model configuration,
  hosted live web search at Codex launch/resume, and inline Source monitor capture passed the
  source-stable `0978bfe1` gate (4,369 tests; 50 skipped). Matching signed native checks passed
  visible/hidden Source capture through the actual Console MCP route, unchanged playheads/selection,
  memory-control placement, and actionable missing-model/key API errors. Source images were delivered
  inline without screenshot files; actual AI interpretation of this new Source fixture is not claimed.
  The replacement demo preserves the existing memory profile and saved session lock; original memory
  and lock hashes also matched after launch.

- **Historical API startup failure: resolved, then API deferred.** The Desktop-only
  `connection_rebind` catalog mismatch was repaired. The signed `b1a36936` package passed native
  dummy-key startup and live owner-key edit/Undo checks. Its three-frame image workflow did not
  complete, so direct API connections are disabled in future builds and remain unresolved rather
  than accepted for alpha. No API voice support is claimed.

- **Desktop reconnect:** the updated helper and explicit `connection_rebind` passed the owner's
  same-chat reconnect check in a new conversation. Older chats whose cached tool catalog lacks
  that tool remain a migration limitation; restarting MCP alone did not recover the tested old chat.
  The owner's JSON handoff to a new chat remains the recorded workaround. Background inspection
  passed native focus checks and owner confirmation that ChatGPT remained in front.

- **Speaking voice, session lock and interruption:** the owner heard Cove and Use Default in
  the matching demo; the original Sol preference was restored. Lock Session during active voice
  preserved the exact conversation and ongoing call. Exact-session reopening and headphone
  interruption passed owner acceptance. No silent new-conversation fallback is allowed.

- **Voice during export:** a representative attended export completed 3,201 frames in 164.4
  seconds with more than 84 seconds of voice overlap. The owner reported small baseline breakup
  that neither improved nor worsened during export, and reports that behavior with Codex rather
  than Desktop. This qualifies no export-induced worsening; the origin of the baseline breakup
  remains unproved and is tracked as a follow-up observation.

- Style-preset native Import and Load passed on a generated preset: the visible selection and
  effective creative context matched exactly, and the timeline/project stayed unchanged. Voice,
  provider access and screenshot files were not involved in that check.

- Input-device discovery/capture recovery was repaired after a signed private app hung
  while opening disconnected Console. A retained sample shows the main thread waiting in
  microphone channel discovery inside CoreAudio. Discovery is now cached on a bounded worker;
  capture startup has a deadline, cancelled callbacks are gated, and Stop/Drop do not join a
  blocked driver. Console and Voice-over settings refresh pending metadata without changing
  unsaved settings. The old-code regression fails as expected; integrated input (13), UI (2)
  and engine (9) tests pass. The full source-stable gate passed (4,203 tests, 50 skipped;
  49 WASM crates). The signed private app passed native Console open/close/reopen with Codex
  disconnected, 12 detected channels and unchanged project/preferences. Physical capture and
  microphone-only mute acceptance later passed in the attended healthy-device scope. Physical
  one-hour endurance and arbitrary driver-failure recovery are not claimed.

- Colored-shadow continuity is qualified in the recorded synthetic CPU/GPU/native scope. A synthetic near-black ramp reproduced a
  large channel jump after negative black-point adjustment and shadow lift. The reference,
  planned-effect and shader paths now share a black-anchored shadow response below 5% of the
  encoded grading signal, preserving the existing weight above that toe. The old-code regression
  failed; ten focused CPU/HDR tests and both actual-device GPU white-balance cases pass.
  The source-stable full gate passed (4,189 tests, 50 skipped; 49 WASM crates);
  signed native Program-monitor review of generated dark ramps also passed at Full and 1/2
  resolution. This isolates a mathematical defect without claiming reproduction of the owner’s exact frame.

- Playback is owner-accepted for alpha; the difficult graded four-view stress case is a later
  roadmap task, not a blocker for the current goal. Nested-placement optimization remains
  documented with its scoped evidence. A paired cold-start measurement
  at 55 seconds showed that removing only opacity masks reduced drops modestly (52% to 46%);
  masks alone are not the remaining bottleneck. The candidate preserves CPU nested composition,
  effects and masks while deferring placement of axis-aligned, full-cover nests to the GPU.
  Actual GPU testing rejected visible rotated/minified edges; those retain CPU placement.
  Retained frame storage is bounded, and upload-cache validation now covers byte limits and
  preservation of every layer in a current batch. The source-stable full gate passed
  (4,185 workspace tests, 50 skipped; 49 WASM crates). Signed builds and matched native
  measurements are complete, but the difficult 55-second interval remains unresolved: about
  60% baseline versus 62% repaired-build video drops at half resolution, with healthy audio
  callbacks and zero underruns. This scoped test does not qualify smooth playback; the owner
  separately reports substantial overall improvement in the demo.

- Recovery working-view restoration is in validation. A two-process abrupt-stop test on a
  private complex project preserved the full project and Undo/Redo, but reopened sequence 11
  instead of the working sequence 1042. Journals now include validated sequence tabs, playheads
  and timeline views, including changes that do not advance the edit revision. The focused
  reproduction passes. A stalled-writer regression also exposed per-frame queue growth; the
  candidate coalesces these into one latest view and one notification per snapshot. Three mailbox,
  22 autosave and six sequence-tab tests pass, with strict engine lint. The rebuilt two-process test
  restores the exact project, working view and Undo/Redo with originals unchanged. The complete
  source-stable gate passed (4,179 tests, 50 skipped; 49 WASM crates). The signed Recovery View
  package also passed native recovery after an owned-process abrupt stop: the saved project/view
  matched exactly, including active sequence and playhead, and recovered Undo/Redo matched the
  expected project states. Physical power-loss durability remains unqualified.

- Larger selected PDFs are in validation: contract 8 adds asynchronous native admission and a
  read-only `reference_pdf` page/ticket tool. Parsing runs in supervised child processes with bounded
  queues, output, input, stream and retained-object/structure budgets. Loaded PDFs remain selected
  after Send until Remove or project switch. The helper indexed the original 216-page Kodak and
  119-page Introduction to Film PDFs and a generated 22.3 MB/448-page illustrated fixture. Focused
  lifecycle/MCP tests, the full source-stable gate and signed app helper checks passed. Native Add
  Files during connected Codex and direct packaged MCP page reads passed, but the later provider
  page request exposed an expired pooled bridge connection. The fresh-connection repair passed
  the full gate and signed native/provider qualification: exact interior/final pages after 130 seconds
  idle, followed by native Remove and stale-snapshot rejection. Native Finder PNG/PDF drop into
  connected Console also passed, including 448-page PDF readiness and an unchanged project.

- Selected PNG reference delivery during voice is in validation: contract 7 adds a read-only
  `reference_image` tool for existing user-selected in-memory snapshots. Stable identities and
  project epochs reject stale requests; bounded PNG delivery retains transparency and creates no
  image files. Focused/transport tests and the full source-stable gate passed, and a separate signed
  package passed native Add Files, packaged MCP pixels after source-copy deletion, and typed-provider
  image interpretation. Native Return/Shift+Return and disconnected draft preservation also passed.
  Actual voice use remains pending.

- Native audio startup safety is in validation: an owned release test blocked the main thread
  in CoreAudio output-device initialization. Output construction, configuration, stream operations,
  recovery and device enumeration now use one guarded bounded worker. The UI reads cached state;
  cancellation silences stale callbacks, and validated program frames drive the clock. Nine synthetic
  blocked-driver/panic/cancellation/format tests, strict Clippy and the full source-stable gate passed.
  A signed native startup/window/menu test passed (3.4-second control readiness; 30 probes within
  13 ms). This is one startup, not repeated driver-failure recovery or playback qualification.

- Live unrendered playback is accepted for alpha; this retained measurement history describes deferred stress limits: a private project copy reproduced heavy video drops
  with the adjustment grade enabled while audio and UI remained responsive. Disabling the adjustment
  reduced but did not remove the drops; it is a diagnostic comparison, not a fix. GPU Noise support
  now preserves the production CPU field, grain size and exact timestamp seed on Metal. Ordered
  adjustment-composite acceleration passed its full source-stable gate and signed native measurement.
  At the same 80-second start and half-resolution setting, the enabled-grade candidate reduced
  dropped frames from 68% to 4% over ten seconds, with zero audio underruns. A full-song run
  measured 6.7% dropped frames, zero audio underruns and zero failed frame jobs, ending naturally.
  Its wrapper timed out waiting for a redundant final review marker; retain that failed harness
  result separately from the completed measurement. Residual drops and transformed-nest CPU
  processing remain open; this is not a claim of universally smooth playback. See [test scope](docs/testing.md#live-unrendered-playback-and-gpu-noise-2026-10-30).

- 2026-10-30 graphics stability: the macOS drawable-timeout policy repair passed the full local
  gate and native QuartzCore property regression, and is packaged separately as Graphics Recovery.
  The signed package passed native hide/restore, background control and resize qualification;
  the original starvation trigger remains unproven. See [evidence and limits](docs/testing.md#bounded-macos-drawable-acquisition-2026-10-30).

- 2026-10-30 Console continuity in validation: Connect chooses the previous saved conversation after
  restarting FilmCraft and rebinds it to the current project context. New Conversation stays explicit. Failed resume
  reports recovery choices and never automatically creates a replacement thread or replays edits.

- 2026-10-30 stability priority: investigate voice expiry followed by short-lived reconnects,
  disabled Voice controls, and failure to restart voice even after Codex reconnect. Owner confirmed
  top priority; source repairs, accelerated provider lifecycle testing and the local combined gate
  passed. The signed local package also passed large-project inspection, edit/Undo, owned Codex-child
  failure and reconnect qualification. A real-provider generated-audio test now passed the actual one-hour expiry, remote Stop
  acknowledgement, worker release, same-conversation voice restart and subsequent text reply.
  Physical microphone/speaker health, native controls and mute continuity remain unqualified.
  See [reported sequence and acceptance](docs/gaps.md#priority-voice-expiry-and-failed-reconnect-recovery-2026-10-30).

- Larger persistent Console memory is in validation following the owner's 16 KB capacity note.
  Contract 9 separates a 1 MiB saved store from bounded session excerpts, UTF-8 read pages,
  literal search and snapshot-guarded small edits. Encoded replies are byte-budgeted; unsaved
  drafts, detected disk conflicts and failed commits preserve the saved notes. The full source-stable
  gate passed (4,173 tests, 50 skipped; 49 WASM crates), as did signed MCP transport checks. Native
  editing, saving and reopening 33 KB of Unicode notes passed an exact saved-byte comparison.
  Owner notes and projects were not migrated or changed.

- Console selection/context-menu repair is in validation following the owner's report that
  right-clicking highlighted output removes the highlight. A pointer-gesture regression reproduced
  the failure. The repair retains egui's shared Unicode/multi-message selection and routes menu
  Copy through it, with whole-message fallback when nothing is selected. All 63 Console tests passed,
  including exact partial/Unicode/multi-message clipboard checks and unchanged draft/project state.
  The full source-stable gate and signed native partial-selection acceptance passed: the highlight
  survived right-click, Copy/Paste transferred only the selected text into the draft without Send,
  and transcript/project state stayed unchanged. Owner projects were not changed.

- 2026-10-30 source-qualified Console contract 6: timeline waveform images with bounded
  time/zoom/identity context, focused visible ranges and no retained screenshot files; microphone-only
  mute and output Copy/Paste into the draft. The full gate passed 4,074 tests (50 skipped).
  The corrected signed Timeline Recovery package passed native/MCP image delivery and actual Codex
  waveform interpretation, plus native Copy/Paste without automatic send. Physical microphone mute
  acceptance remains pending. See [scope and acceptance](docs/gaps.md#planned-ai-timeline-screenshots-for-waveform-inspection-2026-10-30).

- 2026-10-30 diagnostics in validation: bounded command/request/timing/outcome breadcrumbs,
  revision/document identity and fixed Codex setup metadata. Prompts and payloads are omitted.
  Full source checks and the signed package's native diagnostics-header qualification passed.

- 2026-10-30 font previews in validation: shared actual-typeface rows for graphics, captions,
  template controls and font replacement. Temporary font loading avoids permanent byte-cache
  growth; native discovery/rendering use bounded guarded queues and a bounded texture cache.
  Full gates with/without optional fonts passed (4,098 tests each; 49 WASM crates).
  Native Graphics Properties previews, scrolling, selection and Undo passed; other selectors share
  the tested worker and retain their automated coverage. No readiness percentage change.

- 2026-10-30 timeline capture acceptance found a canonical panel-ID mismatch: packaged MCP
  delivered PNG pixels but omitted waveform context and rejected focused ranges. The repair
  shares the actual panel identity between admission and capture. All 4,100 tests and the full gate
  passed. Native full/focused capture, offscreen rejection, unchanged project/view checks and actual
  Codex interpretation of generated waveform intervals passed without retaining screenshot files.

- Export estimate accuracy: explicit finalization phase is implemented for container/audio flush,
  file writes and preview publication. Media-unit counters remain real; finalization has no invented
  zero-second estimate. Focused synthetic-tail, stepped-export/cancellation and UI/queue tests passed.
  The source-stable gate passed. A signed release CLI exported the entire 106.77-second complex
  sequence at 1080×1920/30 fps with audio in 844.78 seconds. Independent decoding read all 3,203
  video frames and 5,005 AAC frames without errors; project/view snapshots stayed unchanged.
  Across 2,586 ETA samples from 10–90% completion, median absolute relative error was 6.76% and
  the 90th percentile was 31.58%. This measures one variable-cost workload, not universal accuracy;
  no workload-specific multiplier was introduced. The owner accepted these ETA limits for alpha;
  further numerical accuracy across workloads is deferred rather than a current goal blocker.

- Console draft keyboard handling is in validation: Return uses the existing Send command,
  Shift+Return adds a newline, and IME composition, held-key repeats and other fields cannot submit.
  Provider failure keeps the draft. The full source-stable gate passed 4,109 tests (50 skipped)
  and 49 WASM crates; native packaged Return/Shift+Return and disconnected draft checks passed.

- Updated Console test-note status: larger PDFs now have bounded asynchronous admission and
  page retrieval, with signed native Add Files acceptance for a generated 22.3 MB/448-page file.
  Native connected-session PNG/PDF drop passed as well. Playback is accepted by the owner for alpha; the
  difficult four-view stress case stays on the later performance list. See the detailed evidence
  and remaining limits above rather than treating the earlier 512 KiB limit as current.

- 2026-10-29 consolidation: the native Command Console is included in local main. Worker completion,
  autosave failure reporting, bounded document reads, sequence identity validation and failed-edit/save
  transactions passed the combined local gate and native/MCP smoke. Benchmark qualification reports incomplete coverage
  and separates decoding, encoding and GPU rendering. These repairs do not change the release stage.

The parity percentages and Opus 5.5 time estimates below are historical estimates from the
2026-10-10 assessment. They have not been recalibrated for the current code or current models.

- 2026-10-29 console qualification: `filmcraft-console/3` in the separate console checkout passed real-Codex import/sequence placement, playhead/selection, direct grouped edits with native Undo/Redo, full-song candidate analysis and Beat Cuts, first-save continuity, reconnect, actual Resume-picker use after restart, and an eight-second H.264/AAC export with all 240 frames decoded. Routine edits need no Apply/Reject. This corrects the earlier limited-command/unsaved-resume record; musical match application/phrase/boundary/drift acceptance and a full-song render remain unverified scopes. No release-stage or readiness-percentage change. See [qualification and limits](docs/testing.md#local-codex-console-qualification).

- 2026-10-25 custom-workflow migration in validation: dedicated Marquee, Timeline Return to Zero, Option-middle zoom, Tools shortcut-editor access, guarded Close All Projects, opt-in persistent Undo and local Flight Recorder. No readiness percentage or release-stage change is claimed. Worker lifecycle/resource-exhaustion repairs are included in validation.

- UI.6: Compact Source and Program monitors separate timecodes from view settings and retain Source drag choices. Transport controls use the shared toolbar sizing helper, with overflow actions in an operable More menu and a scrolling fallback at tiny widths.
- UI.3: Panel tabs, edge splits, tab reordering and floating groups use the shared docking model and drop resolver. Floating geometry is saved with named workspaces while legacy layout JSON remains readable. Dock dividers use bounded shared geometry and keyboard-accessible resize handles. Tools and dock tabs expose actual widget focus, paint focus outlines, and support keyboard traversal/activation. Control input reaches focused widgets in background windows; optional input isolation supports unattended validation. Existing workspace trees and saved layout formats are retained.

**Stage: alpha** · Beta still requires reliable daily workflows and `.prproj` import
([criteria](docs/target-app-parity.md#stage), [core-workflow gate](docs/roadmap.md#alpha-gate)).

> **Last reviewed:** 2026-10-29 (implementation audit; parity estimates 2026-10-10) · **Last updated:** 2026-10-29 · **Change:** major (local consolidation, safety boundaries and current qualification evidence) · **Target:** Adobe Premiere Pro 2026 (26.5.2)

Progress toward parity with Adobe Premiere Pro. This page is the summary; the detail is in
[docs/target-app-parity.md](docs/target-app-parity.md) (assessment, methodology, calibration),
[docs/gaps.md](docs/gaps.md) (the ranked work list: **agents, read it before choosing work**) and
[docs/roadmap.md](docs/roadmap.md) (milestones and current focus).

## Headline numbers

| | Value | How |
|---|---|---|
| **Feature breadth** | **~86%** | Does each Premiere feature exist? Menus 92% (measured: label diff against Premiere 26.5.2's menu dump), effects / transitions / audio effects 100% (measured by tests), other areas estimated |
| **Ready for real work** | **~56%** (51–61%) | How close FilmCraft is to replacing Premiere for an editor on real projects: depth, correctness, file compatibility, speed, stability, ecosystem. Estimated depth per area, combined by a [written formula](docs/target-app-parity.md#three-readiness-numbers) |
| **Mainstream practitioner** | **~55%** | A typical professional editor's weekly work only, discounted ×0.90 interaction, ×0.92 stability, ×0.88 file exchange with Premiere users ([method](docs/target-app-parity.md#mainstream-practitioner)) |
| **Essentials user** | **~60%** | Import, cut, title, music, export at default settings, discounted ×0.88 launch / stability, ×0.90 discoverability, ×0.95 opening files people send ([method](docs/target-app-parity.md#essentials-user)) |
| **To beta** | **~400–700 Opus 5.5 agent-hours** | Estimated, calibrated from this repo's PR history |
| **To full parity** | **~1,550–2,750 agent-hours** | ~70% parallelizes; plugins, AI models, native-speaker review and some hardware need a human |
| Code (measured 2026-10-29) | 50 library crates + 3 apps, ~435k lines of Rust including tests and specification tables; combined local gate: 4,017 tests passed, 49 explicitly skipped | Cargo manifests, Git source inventory, [qualification and coverage limits](docs/testing.md#consolidated-main-validation-2026-10-29) |

Two numbers, because they answer different questions. The gap between them is the work that
matters most now: breadth is nearly done, depth is not.

### Readiness by audience

| Audience | Ready % | Opus 5.5 agent wall-clock hours to ~95% | Work that dominates |
|---|---|---|---|
| Full target (ready for real work) | ~56% | ~1,450–2,600 (~70% parallelizes) | Plugin hosting, AI models, localization incl. RTL / Devanagari, camera RAW and delivery codecs, `.prproj`, hardware acceleration, UI fidelity |
| Mainstream practitioner | ~55% | ~500–850 (~65% parallelizes) | `.prproj` import (60–120), UI fidelity (120–200), stability and PR CI (40–80), 10-bit / 4:2:2 camera decode (30–50), playback / hardware (60–100), import and export gaps (60–100) |
| Essentials user | ~60% | ~180–350 (~60% parallelizes) | Launch crashes on Windows / Intel / older macOS (40–80), discoverability and timeline friction (60–110), playback lag (30–60), common import gaps (20–40), audio devices (15–30) |

Method and calibration: [docs/target-app-parity.md](docs/target-app-parity.md#three-readiness-numbers).

## By dimension

| Dimension | Ready | Hours | Doc |
|---|---|---|---|
| Features (editing, effects, colour, audio, graphics, export settings) | ~66% | 350–620 | [target-app-parity.md](docs/target-app-parity.md#by-feature-area) |
| UI / UX fidelity | ~60% | 120–200 | [ui-parity.md](docs/ui-parity.md) |
| File formats (containers, `.prproj`, interchange, captions, stills) | ~60% | 140–240 | [file-format-parity.md](docs/file-format-parity.md) |
| Codecs | ~60% | 150–250 | [codec-parity.md](docs/codec-parity.md) |
| Hardware acceleration | ~45% | 120–200 | [hardware-parity.md](docs/hardware-parity.md) |
| Localization | ~35% | 150–230 | [localization-parity.md](docs/localization-parity.md) |
| Performance | ~55% | 60–120 | [performance.md](docs/performance.md) |
| Stability | ~45% | 60–120 | [gaps.md G4](docs/gaps.md#g4-launch-crashes-and-cross-platform-pr-ci) |
| Platforms (macOS, Windows, Linux, FreeBSD, web) | ~60% | 50–90 | [gaps.md G9](docs/gaps.md#g9-windows-and-linux-at-run-time) |
| Plugins and ecosystem | ~5% | 200–400 | [gaps.md G11](docs/gaps.md#g11-no-plugin-hosting) |
| AI features | ~25% | 150–300 | [gaps.md G12](docs/gaps.md#g12-ai-features) |

## Features

| Area | Breadth | Ready | Hours |
|---|---|---|---|
| Editing, timeline, trimming, multicam | ~90% | ~70% | 80–140 |
| Effects and transitions | 100% (measured) | ~60% | 80–140 |
| Media import and codecs | ~75% | ~60% | 150–250 |
| Audio | ~88% | ~65% | 50–90 |
| Panels and UI fidelity | ~88% | ~60% | 120–200 |
| Colour | ~90% | ~70% | 40–70 |
| Graphics, titles, captions | ~85% | ~65% | 50–90 |
| Export and delivery | ~80% | ~65% | 60–110 |
| Project files and interchange | ~70% | ~45% | 90–160 |
| Preferences and project management | ~80% | ~65% | 20–40 |

Evidence for every row: [docs/target-app-parity.md](docs/target-app-parity.md#by-feature-area).

## Languages

Premiere ships 10 interface languages. Measured from our catalogs (3,498 strings):

| Language | Status | Strings |
|---|---|---|
| English | full | 100% |
| Simplified Chinese | partial (whole interface; engine errors English) | 99.6% |
| Spanish | partial (whole interface) | 99.9% |
| Hindi | none (no Devanagari shaping in the interface) | 0% |
| Arabic | none (no RTL interface) | 0% |
| French | partial (whole interface; native review welcome) | ~100% |
| Portuguese (Brazil) | partial (whole interface; native review welcome) | ~100% |
| Indonesian | partial (whole interface; native review welcome) | ~100% |
| Italian | partial (whole interface; native review welcome) | ~100% |
| Korean | partial (whole interface; needs an installed Hangul font; native review welcome) | ~100% |
| Japanese | partial (whole interface) | 99.7% |
| German | partial (whole interface; menus native-reviewed, the rest welcome) | ~100% |
| Vietnamese | none | 0% |

Also shipped: Ukrainian (menus only, 10%). Detail: [docs/localization-parity.md](docs/localization-parity.md).

## Upcoming

Ranked; detail and "done when" in [docs/roadmap.md](docs/roadmap.md) and [docs/gaps.md](docs/gaps.md).

1. **No launch crashes, and verified CI on pull requests** (macOS 12, Intel UHD, Windows 11 reports; the three-platform gate needs live runner results). 40–80 h.
2. **Open Premiere's `.prproj`**, the beta blocker. 60–120 h.
3. **10-bit / 4:2:2 camera media without hardware** (H.264 High 10 / 4:2:2, HEVC 4:2:2; #626). 30–50 h.
4. **Measure fidelity against Premiere** and add `cargo xtask parity`. 50–90 h.
5. **Real-world media corpus and workflow acceptance tests.** 40–70 h.
6. **Hardware acceleration, continued** (#30): zero-copy upload, more effects on the GPU, Quick Sync / AMD encoders, VideoToolbox Main 10. 120–200 h.
7. **UI fidelity:** Program monitor transform handles, Effect Controls, docking. 120–200 h.
8. **Plugin hosting** (VST3 / AU, then OpenFX). Owner decision needed. 200–400 h.

## Progress log

- **2026-10-22:** ProRes 4444 / XQ alpha slices include the final singleton run code required by RDD 36:2022. Six HD fixtures rejected by native macOS decoding now pass; syntax regression tests cover final runs and slice widths ([details](crates/prores/README.md#alpha-bitstream-conformance), EffectCraft #293).

- **2026-10-12:** Transform's motion-blur shutter on adjustment layers uses the sequence frame rate instead of 24 fps, so a 360° shutter in a 60 fps sequence spans one frame, not 2.5 (#1088)
- **2026-10-10 (changed media):** Settings ▸ Media ▸ Automatically refresh growing files works: media files that change on disk while the project is open (a recording still being written, a render written over the file a clip uses) are read again every *Refresh growing files every* seconds and as soon as the window comes back to the front: the new length, picture and identity, without an undo step (`media.refreshChanged`, `media_watch`).
- **2026-10-10 (re-measure):** full parity re-measure against Premiere Pro 26.5.2: menu label diff 338 / 366 (92%), installed bundle listings (10 UI languages, exporter and codec frameworks), 188 open issues. Breadth ~86% (file formats now counted), ready ~55%, stage **alpha**; progress docs reorganized per the craftrules standard ([target-app-parity](docs/target-app-parity.md), [gaps](docs/gaps.md), [roadmap](docs/roadmap.md), [codec](docs/codec-parity.md), [file-format](docs/file-format-parity.md), [hardware](docs/hardware-parity.md), [UI](docs/ui-parity.md), [localization](docs/localization-parity.md)).
- **2026-10-10:** ~75 community PRs merged: GPU export rendering (Auto) with Vignette, Video Limiter and Lumetri basic / creative / vignette on the GPU (#421); Linux NVDEC H.264 / HEVC decode (#549); text-to-speech narrations with Kokoro-82M (#278); Japanese for the whole interface (#449) and Ukrainian menus (#407); ALSA default-device fallback (#254, fixes #23); drop import into the timeline (#517); system appearance (#505); timecode scrub editing (#576); NVENC RGBA input (#486); marker CSV export (#454); many fixes (#526–#575).
- **2026-10-09:** release 0.5.0. NVENC H.265 export on Windows, Main and Main 10 HDR (#328, #360); NVENC H.264 export (#386); VA-API H.264 / HEVC decode (#358); Source monitor playback (#423); multi-stream container audio, schema v13 (#361); New Sequence and Sequence Settings dialogs (#269, #355); Spanish for the panels (#238); background decoders and memory stress tests (#385, #381).
- **2026-10-08 (Source range dragging):** implicit full-clip In/Out, video-only/audio-only/linked both controls, picture/waveform drag gestures and one-command undoable timeline placement. Shared Source/Program draggable frame-bounded handles and range translation, cancellation, Source marker/navigation routing and marked-range playback/looping.
- **2026-10-08 (Source playback):** normal forward playback for video/audio media and subclips in the Source monitor, an independent Source clock, Play/Pause and focused Space, frame stepping, device-clock audio with wall-clock fallback, seeking, and end-of-clip stopping. Windows desktop review verified a real video file and standalone WAV, including In/Out marking. Reverse/shuttle Source playback and source sequences remain unsupported; no parity percentage changed.
- **2026-10-08 (night):** HDR H.265 (HEVC Main 10, PQ and HLG, BT.2020, limited range) export on Windows through NVENC (#30): an HDR sequence now exports as real 10-bit HDR (`rgbf_to_yuv420_10` into P010 input buffers; VUI BT.2020 + PQ / HLG; the HDR10 mastering-display and content-light messages as SEI on every IDR for PQ, through the generic SEI payload array because NVENC 12.1 has no mastering-display field; `colr` / `mdcv` / `clli` in the sample entry), where the GPU reports 10-bit HEVC (new `register_hdr_probe` / `hdr_available` in the export crate; elsewhere, and on macOS, HDR sequences still tone-map to 8-bit SDR HEVC, and `settings.sdr` forces that). ffprobe reads `hevc` / `Main 10` / `hvc1` / `yuv420p10le` / `bt2020` / `smpte2084` or `arib-std-b67` / `bt2020nc` / `tv` with the mastering-display and content-light side data for PQ; ffmpeg's decode equals ours code for code; the in-process round trip is at 71.1 dB luma PSNR on the 10-bit scale with 803 distinct levels on a ramp and the peak code 940 intact (RTX 5060). 8-bit SDR H.265 is unchanged.
- **2026-10-08:** H.265 (HEVC Main, 8-bit 4:2:0, SDR) export on Windows through NVENC (#30): the NVENC session, ring and NV12 path of the H.264 encoder now serve both codecs (the H.264 export is byte-for-byte unchanged); the format is available where the GPU has an HEVC encoder, choosing it is the opt-in, and `hvcC` is built from the SPS the encoder wrote. ffprobe reads `hevc` / `Main` / `hvc1` / `yuv420p` / BT.709; our own HEVC decoder decodes it at 48.9 dB luma PSNR (RTX 5060). Main 10 and HDR remain.
- **2026-10-08 (#285):** `filmcraft-cli` no longer panics with "Broken pipe" when its stdout is closed early (`filmcraft-cli commands | head`): later output is dropped, the rest of the work (script lines, `--save`, `--save-as`) still runs, and the exit status still reports failures. Other stdout write errors are reported and make the exit status 1. No change to the checklist or estimates.

- **2026-10-06:** 21 community PRs landed (#19, #28, #38, #39, #41–#60). Highlights: hardware decoding is now actually on in the desktop app (#41: `register()` sat inside a `log::info!` that never ran, so #33's VideoToolbox path only worked in the CLI and benchmarks); decode memory on long timelines cut sharply (frame pool, shared GOP-cache budget, positional file reads); exports are byte-identical across machines (#19); crash fix for odd track names over MCP (#44); ripple trim, ripple delete and speed changes move split-edit clips once (#56–#58); Japanese interface using an installed system font (#38; no fonts bundled); MCP annotations, resources, export progress and cancellation (#28).

- **2026-10-05:** honest assessment added (checklist ~87% vs ready for real work ~50–60%). Hardware acceleration started (#30): blend modes on the GPU (#32); VideoToolbox decode + `platform` crate, the one crate allowed `unsafe` (#33). Community fixes landed: AIFF import, export start/end ranges, transitions following ripple trims, Slide moving linked audio (#5–#8).

- **2026-10-04 (later):** interchange + MXF export (M11.4/M6.6): own compound-file container (`crates/cfb`), AAF Edit Protocol import/export (embedded/linked/consolidated audio, video mixdown, handles, breakout to mono), OMF 2.0 export (Bento), MXF OP1a/OP-Atom writer (DNxHR/ProRes/H.264/PCM) as export formats. 1605 tests.

- **2026-10-04:** MPEG-2 (M9.15): own MPEG-2/MPEG-1 video decoder (4:2:2, interlaced, field pictures; IEEE 1180 IDCT), MPEG TS/PS demux (AVCHD .mts, .m2ts, .ts, .mpg/.vob/.mod), own AC-3 decoder (ATSC A/52), MP2, LATM AAC, Blu-ray/DVD LPCM, MPEG-2 in MXF/MOV/MP4/MKV. 1562 tests.

- **2026-10-03 (evening):** Project panel and Media Browser (M12.7): Freeform view (stacks, arrangements), Icon view hover scrub with In/Out, list columns from all metadata, ten view presets, multiple Project panels; Media Browser with Favorites/drives/recent, back/forward, filters, thumbnails, Edit Columns, ingest, drag to timeline; all as `project.*` / `mediaBrowser.*` commands. 1513 tests.

- **2026-10-03 (later):** graphics & captions (M10.7): `.fcgt` graphics templates (export/install/apply/edit, 8 original built-ins; Adobe .mogrt refused), Essential Graphics Browse/Edit, rolls/crawls, responsive design pins and time, per-character styles, Upgrade Caption to Graphic / to Source Graphic, MCC/EBU STL/TTML/DFXP, Replace Fonts; schema v12. 1479 tests.
- **2026-10-03:** 4K H.264 playback (M4.9): vectorised deblocking, branch-free CABAC bins and column-parallel inverse transforms cut H.264 decoding by 23 % (cycles per frame, bit-exact; 4K cold seeks −27 % CPU); opt-in draft decoding (Settings ▸ Playback) skips deblocking of non-reference pictures and uploads decimated planes while playing at 1/2–1/4, never for paused frames or exports; conformance runs 1, 3 and all threads. 4K plays 192/0 at load 100–140 (before: only at load ~90).
- **2026-10-03:** 4K VP9 / HEVC / AV1 playback (M4.10): VP9 frame threading (references published per superblock row, loop filter overlapping the next frames) plus a vectorised loop filter, column-parallel transforms and 16-bit MC cut VP9 work by 25 % single-threaded and 44 % with all cores (4K decode 8 → 27–34 fps at load ~200); HEVC −28 % (vectorised SAO, MC without per-block zeroing, batched bypass bins), AV1 −20 % (fixed-width MC, recycled planes); opt-in draft decoding for all three; bit-exact at 1, 3 and all threads. 4K VP9 and HEVC now play 192/0 at load ~200 (before: 0–58 of 192 shown).

- **2026-10-03:** keyboard parity (M3.12: 143 new commands on Premiere's default keys, table validated against the registry), audio (M7.8: 5.1 tracks/submixes/Mix, 5.1 panner, BS.775 downmix, 5.1 WAV/MOV/AAC export, voice-over record with punch-in, Remix, Clip Mixer Latch/Touch/Write). 1432 tests.

- **2026-10-02 (late):** export parity (M6.5): built-in preset library + user presets, full Export-mode settings (loudness normalization, image/name/timecode overlays, captions, range), export queue, Quick Export, Premiere-style image-sequence export, `filmcraft-cli export --preset`; media import (M9.14): MXF OP1a/OP-Atom (new `crates/mxf`, H.264/DNx/ProRes/PCM, timecode), Ogg Opus (new `crates/ogg`), image sequences as clips, BWF timecode. 1373 tests.

- **2026-10-02 (panels):** complete Lumetri Scopes (new `filmcraft-scopes` crate: Vectorscope YUV/HLS, Histogram, Parade RGB/YUV/RGB-White, Waveform RGB/Luma/YC/YC no Chroma; wrench menu with presets, colour space, brightness, scale; < 1 ms per scope at 1080p; `scopes.read` returns the numbers); Metadata panel (editable log fields, undoable `metadata.set`), Timecode panel, Events panel (event log of failed commands, jobs, auto-save), Progress panel (jobs with Cancel), Reference Monitor (parked or ganged, picture or scopes).
- **2026-10-02 (evening):** performance (M4.8): `cargo xtask bench` (decode, seek, playback, scrub, timeline UI, export, project I/O, memory; results in docs/performance.md) and `perf.stats` for agents; HEVC transform/SAO 2.5× cheaper; late frames skip non-reference pictures (4K CPU per displayed frame halved, cold seeks 20–45% faster); decoders stay bit-exact. 1270 tests.

- **2026-10-02 (later):** all 93 video effects (+Legacy/Obsolete bins), all 53 audio effects with Parametric/Graphic EQ, Multiband Compressor and Dynamics editor windows, all 84 video transitions (+21 Legacy), Settings dialog (16 categories, most wired), remaining menu items (Scene Edit Detection, Find, Normalize Mix Track, Simplify Sequence, Automate to Sequence, search bins, templates, Project Settings with scratch disks, ALE and selection-as-project export, system report); project schema v11. 1258 tests.

- **2026-10-02:** Premiere menu long tail: Sequence/Markers (gaps, split edits, through edits, subsequence, Delete Tracks, range/chapter markers, ripple sequence markers), Clip/Edit/File (Paste/Remove Attributes, subclips, Frame Hold Options, Time Interpolation with frame blending, Audio Channels, Breakout to Mono, Extract Audio, Replace With Clip, Remove Unused, Consolidate Duplicates), View/monitors (paused resolution, alpha/RGB display modes, comparison view, magnification, rulers, guides + templates, snapping), Graphics and Titles (vertical text, shape layers, align/distribute/arrange). Transcripts and text-based editing (Text panel, optional local Whisper). AV1 tile/frame/post-filter threading. Agent-friendly CLI (`exec`, `inspect`, `describe`, `import`, `export`, `run -`, `--save`, `--bridge`). Project schema v10.

- **2026-10-01 (night):** new README hero (Apollo 11 documentary edit, NASA public domain); agents never steal keyboard focus; masks + tracking, adjustment layers, effect presets; multicam + audio sync; DNxHD/DNxHR decode/encode; AV1 decoder bit-exact against libdav1d (all stages, film grain, superres, SVC).

- **2026-10-01 (M12 multicam):** Synchronize / Merge Clips / Create Multi-Camera Source Sequence (sync by In, Out, timecode ± hours, clip markers, or audio: GCC-PHAT cross-correlation, offsets recovered to the sample on noisy multi-mic recordings), multi-camera clips (angle render, Switch Audio), Program monitor Multi-Camera view with live switching (keys 1–9, one undo step per pass), Ctrl+1–9 cuts, Enable / Flatten / Edit Cameras, project schema v7. Fixed: nested sequence audio was silent.
- **2026-10-01 (evening):** M11 done: offline media with our own slate and the Link Media dialog (fingerprint-checked relink, folder remap, search), proxies (create in background / attach / toggle; export stays full-res) with ingest settings, Project Manager (collect, consolidate + transcode). Fixed: GOP cache deadlock when a rayon decoder inside a parallel export re-entered the same source.
- **2026-10-01 (M7.4):** Essential Sound panel: Dialogue/Music/SFX/Ambience types, Loudness auto-match (BS.1770, exact to the target), Repair (noise, rumble, hum, new DeEsser and spectral DeReverb), Clarity (dynamics, EQ presets, Enhance Speech DSP chain), Creative reverb / stereo width, ducking that writes Volume keyframes, presets; all `essentialSound.*` commands, effects visible and keyframable in Effect Controls.
- **2026-10-01 (night):** M8 colour: camera log curves and gamuts from published specs, colour-managed pipeline (PQ/HLG working spaces, Interpret Footage, BT.2390 tone mapping), HDR export signalling verified with ffprobe, LUTs (.cube/.3dl, tetrahedral CPU + WGSL, project library, Lumetri Input LUT / Look), Colour Match, Lumetri section bypass, HDR scopes.

- **2026-10-01 (night):** DNxHD/DNxHR decoder + encoder (SMPTE ST 2019-1) with MOV export; AV1 decoder (spec v1.0.0 + Errata 1, all decoding tools incl. film grain) bit-exact against libdav1d, wired into MP4/WebM/MKV import with key-frame-checked seeking.
- **2026-10-01 (later):** text engine (shaping, bidi, line breaking) + Type/Shape/Pen tools + graphic clips + graphics panel; Trim Monitor + dynamic trimming; Keyboard Shortcuts editor; audio mixer with automation. Fixed: MP4 muxer wrote unreadable all-empty sample tables; system font scan race.

- **2026-10-01:** Audio mixing (M7.2/M7.5/M7.6 basics): mixer graph with submixes, sends, inserts and latency compensation; Audio Track / Clip Mixer panels; Latch/Touch/Write automation recorded live; timeline track keyframes; Audio Gain dialog.
- **2026-10-01:** recovered from a machine crash with no lost work. Merged: VP9 decoder (profiles 0–3, bit-exact, WebM/MKV/MP4), Opus decoder (RFC 8251 range-exact; WebM/MKV/MP4), captions (SRT/VTT/SCC, burn-in), render bar + render previews, project schema versioning + atomic saves + auto-save + crash-recovery journal, test infrastructure (golden images, loudness oracle vs ffmpeg, headless scripted UI tests; fixed a GPU stale-texture bug), public contributor docs and licence files.

- **2026-09-30 (evening):** Opus decoder (RFC 6716/8251, range-exact on every conformance vector, ~80–110× realtime 48 kHz stereo) wired into WebM/MKV and MP4 import.
- **2026-09-30 (afternoon):** Matroska/WebM import; LUFS meters; clip audio effects on the DSP crate.
- **2026-09-30 (midday):** HEVC decoder, Matroska/WebM demuxer and audio DSP merged; HEVC import wired; asset rules (AGENTS.md, ATTRIBUTION.md, `cargo xtask assets`); README with hero screenshot and the Craft family.
- **2026-09-30 (late morning):** keyframe value/velocity graphs; xtask gates; H.264 MP4 export; Lumetri curves, wheels, looks, HSL secondary.
- **2026-09-30 (early morning):** H.264 decoder, ProRes, AAC; GPU compositor; MCP; Premiere 26 visual fidelity pass.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | minor | ProRes alpha: write the final singleton run code; 4444/XQ HD clips that failed native macOS decoding now decode, with syntax regression tests (EffectCraft #293) |
| 2026-10-13 | minor | Language table: German covers the whole interface (~100%) |
| 2026-10-12 | trivial | Progress log: Transform shutter on adjustment layers (#1088) |
| 2026-10-11 | minor | Language table: German ships the menus (#630, 10.6%) |
| 2026-10-10 | minor | Readiness-by-audience table with hours to ~95% per audience |
| 2026-10-10 | minor | Headline adds mainstream practitioner (~55%) and essentials user (~60%); ready for real work ~55% -> ~56% (formula written out, same inputs); beta distance ~19 points |
| 2026-10-10 | minor | Stage banner links the core-workflow gate (all six workflows pass on macOS; stays alpha) |
| 2026-10-10 | major | Restructured to the craftrules progress-docs shape: stage banner, headline numbers, dimension / feature / language tables, upcoming; re-measured against Premiere 26.5.2 (breadth ~87 → ~86%, ready ~50–60% → ~55%); honest assessment and scorecard moved to docs/target-app-parity.md, "Where we are lacking" to docs/gaps.md, milestones to docs/roadmap.md, interface localization to docs/localization-parity.md |
| 2026-10-08 | minor | Log entries for HEVC export, Source playback and range dragging |
| 2026-10-05 | major | Honest assessment added: checklist ~87% vs ready for real work ~50–60% |
