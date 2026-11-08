# Testing

```sh
cargo test --workspace                          # everything; oracle tests skip without ffmpeg
cargo nextest run --workspace                   # the same in parallel processes (doctests: cargo test --doc)
cargo test -p filmcraft-h264                    # one crate
cargo test --release -p filmcraft-hevc          # codec tests are much faster in release
cargo xtask ci                                  # all gates (fmt, clippy, tests, layers, assets, wasm)
cargo xtask fixtures                            # pre-generate the ffmpeg fixture matrix
FILMCRAFT_REQUIRE_ORACLES=1 cargo test --workspace   # CI: missing ffmpeg fails instead of skipping
```

Integration tests build one binary per crate: a crate with several test files keeps them in
`tests/it/`, and `tests/it/main.rs` declares each as a module. Add a test file there with a `mod`
line in `main.rs`; shared helpers live in `tests/it/common/` or `tests/it/support/`. A test file that
changes process-wide state (a global switch, pool, panic hook or fault injection) or times a workload
stays a binary of its own in `tests/`. Select one file's tests with its module path:
`cargo test -p filmcraft-h264 --test it conformance::`.

## 1. Kinds of tests

2026-10-29 stabilization regressions cover AV1 failures before decoding and after setup (both
output/reference slots, dependent jobs, the next independent job and bounded decoder shutdown),
auto-save worker failure with persistent notification and successful manual saving, and media-scan
failure clearing its pending slot. The AV1 inline regression fails with an injected panic when
the completion guard is removed; the fixed threaded and no-default-features paths are checked.

Project/format tests reject sequence-local identity collisions through current and migrated
files while preserving legal sequence copies and harmless overlaps. Engine tests check generic
and merged edit rejection, damaged undo snapshots preserving existing history, sparse oversized
project/recovery/LUT files, failed Save As, and rejected groups preserving cleanliness, merge
state, events and a full command journal without publishing intermediate recovery snapshots.
Timeline analysis tests keep one worker reservation across rapid requests/cache resets and
release it after an injected panic. Benchmark example tests run with
`cargo test -p filmcraft-ui-egui --example bench`; required runs reject missing/failed scenarios
and CPU mode fixes decoding, encoding and compositing to software independently of other flags.

| Kind | Where | Examples |
|---|---|---|
| Unit | `src/` `#[cfg(test)]` modules, `src/tests.rs` | CABAC engines, VLC tables (prefix-freeness, Kraft sums), colour matrices, frame cache |
| Property (`proptest`) | `time`, `bitstream`, `edit`, `isobmff/tests/it/roundtrip.rs`, `interchange/tests/it/*` | tick↔frame round trips for every rate, DF timecode, edit invariants (no overlaps, durations conserved), mux→demux round trips, interchange export→import |
| Synthetic streams | `h264/src/synth_tests.rs`, `hevc/src/synth_tests.rs` | hand-built bitstreams for features the reference encoders don't emit; expected output known exactly |
| Scripted UI | `crates/ui-egui/tests/it/scripted.rs` | headless app (egui_kittest) driven over the control channel: razor/undo, insert, apply effect, playback, click by automation id (§4) |
| Native file dialogs | `crates/ui-egui/tests/it/native_dialogs_ui.rs`, `apps/filmcraft/src/main.rs` | asynchronous import and Save As continuations, cancellation and one-dialog-at-a-time behavior; on macOS the desktop runner must use an asynchronous sheet so a file picker cannot reenter winit's active event handler |
| Monitor view / Graphics menu | `crates/ui-egui/tests/it/view_ui.rs`, `crates/engine/src/graphics_tests.rs` | View menu items and checkmarks, playback/paused resolution, channel / comparison / multi-camera / waveform display modes, magnification and Hand-tool panning, guides dragged from the rulers (move, lock, remove, Add Guide…, templates in the preferences), snapping a graphic to the frame centre and a guide; align to frame / as group / to selection, distribute (centres and gaps), arrange, select next/previous graphic and layer, reset parameters / duration, vertical text, New Layer from file (`FILMCRAFT_UI_SHOTS=<dir>` writes `view-*.png`) |
| Settings | `crates/engine/src/settings_tests.rs`, `crates/ui-egui/tests/it/settings_ui.rs` | schema keys and defaults, persistence in the data directory, v1 → v2 migration and repair of bad values, validation, per-category reset; each wired setting (still / transition durations, step many, label defaults and colours, media scaling, timebase, recent projects, media cache policy, output mapping, smart quotes, auto-transcribe); the dialog: every category from its command and Cmd+,, edits by automation id, OK / Cancel / Escape / Reset…, theme, label names in Edit ▸ Label, tooltips, frame cache (`FILMCRAFT_UI_SHOTS=<dir>` writes `settings-*.png`) |
| Scopes and panels (M8.9 / M12.6) | `crates/scopes/src/tests.rs`, `crates/engine/src/{scopes,panels}_tests.rs`, `crates/ui-egui/tests/it/panels_ui.rs` | scope maths on generated frames (flat colours → exact histogram bins, ramps → waveform row = code value, 75 % bars → parade levels and vectorscope target cells for BT.601/709/2020, HLS hue angles, YC chroma, Clamp Signal, decimation, NaN); `scopes.read` on a colour matte (exact bins, 78.43 %) and on Bars and Tone (the six targets); metadata edits (one undo step, read-only fields refused, saved in the project), the event log (failed / disabled commands, repeats, jobs started / finished / failed / cancelled); every panel from Window ▸, the scopes' wrench menu, presets, five-scope grid, typing into a Metadata field, Timecode rows and modes, Events filter / Clear All, Progress cancel, Reference Monitor park / gang / scopes (`FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `panels-*.png`); `perf_scopes_at_1080p` (ignored) times each scope |
| Keyframe navigator and Effect Controls ruler | `crates/engine/src/tests.rs` (`add_keyframe_toggles_the_keyframe_at_the_playhead`), `crates/ui-egui/tests/it/keyframes_ui.rs`, `lane_tests` in `crates/ui-egui/src/panels/effect_controls.rs` | `effects.addKeyframe` adds a keyframe at the playhead or removes the one there (the value falls back to the remaining keyframes; undo, redo, parameters that name nothing); headless: the Properties diamond turns animation on, adds and removes the keyframe at the playhead (Scale 50 % → 100 % again), its ◀ ▶ arrows and Effect Controls' step between keyframes, Crop is applied before its first keyframe; a click on the Effect Controls time ruler and a drag of the playhead's handle move the playhead, and no playhead is drawn off the clip; ruler labels are sequence timecode, and clip times, frame rates and panel widths at their limits don't panic (`FILMCRAFT_UI_SHOTS=<dir>` writes `keyframes-*.png`) |
| Golden images | `crates/golden/tests/golden.rs` | CPU renders vs committed PNGs; GPU vs CPU on the same scenes (§3) |
| Engine / command | `crates/engine/src/tests.rs` | run commands on the demo project, assert the sequence, undo/redo, disabled cases |
| Render | `crates/render/src/tests.rs` | compositing, opacity, Motion, cross dissolve midpoint, ½-res vs full, GPU plan vs reference, audio mix, audio-effect continuity |
| Multi-camera / sync | `crates/audio-dsp/src/sync.rs`, `crates/engine/src/multicam_tests.rs`, `crates/ui-egui/tests/it/multicam_ui.rs` | audio offsets within one sample: Rust-generated multi-mic speech (gains −20…+10 dB, SNR 40/25/10/3 dB, zero-phase colouring, fractional delays, a 0 dB-SNR room with a −4 dB echo) and ffmpeg-made camera/recorder files (pink-noise bursts, independent noise; exact lags recovered); timecode (± hours) and marker sync within a frame; Merge Clips; timeline Synchronize keeps frame-aligned starts; multicam structure, angle render (PSNR vs the camera), Switch Audio mix = the angle's track, live switching from a simulated playback clock (cuts, healing, one undo step), Ctrl+N cuts, flatten (frames identical), save/load; 20 angles: paging (auto / 2×2 / 3×3), page-relative keys, absolute cameras beyond 16, a page decoding only its angles at the cell scale (recording source), Auto-Adjust scale, Selection Top Down; headless: page arrows, angle 17–20 cells, grid-only view, Edit Cameras thumbnails; `perf_grid_of_four_1080p_angles` (ignored) prints the grid cost |
| Project panel and Media Browser (M12.7) | `crates/engine/src/{project_panel,media_browser}_tests.rs`, `crates/ui-egui/tests/it/project_panel_ui.rs` | view settings and their preference round trip; Metadata Display columns (custom fields, Freeform keys hidden), numeric vs text sorting, view presets (save / save as / restore / rename / delete, ten slots, persisted in `preferences.json`); Freeform moves keeping offsets, snapping, Align to Grid, Clip Size, stacks, arrangements, Reset (each one undo step, saved in the project); bin rename / nesting; the Media Browser against a fake filesystem (folders first, hidden and unsupported files left out, file-type filter, back / forward / up, recent directories, Favorites, import of a selection or a folder, Open in Source Monitor, ingest, probe columns). Headless: List view (sort by header, drag a column wider, inline rename), Icon view (hover scrub + I / O set the clip's In / Out, Shift+] / [), Freeform (drag a card, stack from the card menu, options dialog), panel menu, Metadata Display dialog, view presets, bins in place / tab / window, footer buttons, Media Browser navigation, Favorites, import, Shift+O, Edit Columns…, thumbnails, drag to the Timeline (`FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` writes `project-*.png`, `media-browser-*.png`) |
| Menu long tail (M3.11) | `crates/render/src/scene.rs`, `crates/engine/src/{scene_detect,sequence_extras,project_tools}_tests.rs`, `crates/interchange/src/ale.rs`, `crates/project/src/find.rs`, `crates/ui-egui/tests/it/menus_ui.rs` | Scene Edit Detection on synthetic frames (hard cuts vs pans, flashes, sensitivity) and on generated movies with known cut points (our own ProRes render of demo scenes; an ffmpeg-made H.264 concat of testsrc2 / SMPTE bars / Mandelbrot): cuts with linked audio, clip markers, subclips, one undo step, background job and cancel; Normalize Mix Track to a target peak; Simplify Sequence; Find / Find Next and search bins; Automate to Sequence (overlap, transitions, insert, unnumbered markers); ALE round trip; Project Settings and scratch disks; menu order and shortcuts against Premiere's; the dialogs by automation id |
| Mixer | `crates/render/src/mixer_tests.rs`, `crates/engine/src/mixer_tests.rs` | sample-exact fader gain and pan laws, automation at block boundaries (bit-identical however requests are cut), solo/mute/solo-safe, sends and submix routing, latency-compensation alignment, render-vs-playback identity, Touch ramp-back, recorder modes (Latch/Touch/Write), thinning, Audio Gain, transition curves; `perf_24_tracks_3_effects_realtime_factor` prints the realtime factor |
| 5.1 / multichannel | `crates/audio-dsp/src/channels.rs`, `crates/render/src/mixer_tests.rs`, `crates/export/src/surround_tests.rs`, `crates/engine/src/mixer_tests.rs`, `crates/ui-egui/tests/it/audio51_ui.rs` | BS.775 downmix / upmix coefficients exact, mixdown types; 5.1 panner point gains equal-power (Σg² = 1 over a 21×21 grid × 3 centre settings), speaker positions exact, 5.1 through the default puck bit-exact identity; graph: stereo/mono tracks into a 5.1 Mix (L/R, C, rear right, room centre −6 dB each, LFE = mean), 5.1 track through a 5.1 Mix and folded into stereo, a 5.1 submix, `pan51` automation block-invariant, six-channel meters; export: 6-ch WAV (`WAVE_FORMAT_EXTENSIBLE`, mask 0x3F) and ProRes MOV (6 × 24-bit PCM) decoded by ffmpeg **sample-exact**, ffprobe `channels` 6 / `channel_layout` 5.1; AAC 5.1 in MP4: every decoded channel's own tone ≥ 30 dB above the others' (no swapped channels), stereo export of a 5.1 Mix = BS.775 within 2·10⁻⁶; `sequence.settings {mix}`, `file.newSequence {mix, trackType}`, panner lanes, Write pass records the puck; UI: puck drag (one undo step), Center / LFE, six meter bars |
| Audio Clip Mixer automation, Track Mixer view | `crates/engine/src/mixer_tests.rs`, `crates/ui-egui/tests/it/audio51_ui.rs` | Touch: a 201-point fader ramp thins to ≤ 6 clip keyframes, values within 0.1 dB, live override while held, values outside the gesture unchanged, one undo step; Latch holds until stop; Write records volume and pan from the start; a gesture across two clips writes both; Read commits once without a pass; UI: Clip Mixer mode dropdown, a recorded fader drag writes clip keyframes; Show/Hide Tracks, Meter Input(s) Only, transport record button |
| Voice-over | `crates/engine/src/voiceover_tests.rs`, `crates/ui-egui/tests/it/voiceover_ui.rs` | synthetic click train lands in `mix_sequence` exactly at R + k·period (pre-roll dropped, exact sample count), punch-in between In/Out (ramp source, every sample checked), one undo step + undo/redo, take numbering, record-arm/targeting track choice, sync/discard/zero-length/locked cases, cue times; UI: right-click Mic → Voice-Over Record Settings dialog (edits persist on OK, Cancel discards), the Mic button starts and stops a take on its track with pre-roll and overlay, and records past the end of the timeline (also `voiceover_playback_tests` in `crates/ui-egui/src/lib.rs`) |
| Text to Speech | `crates/tts/src/**/*.rs` (Kokoro tests need `FILMCRAFT_KOKORO_DIR`), `crates/tts-text`, `crates/engine/src/narration_tests.rs`, `crates/ui-egui/tests/it/tts_ui.rs` | built-in voices: determinism, finite samples, exact-silence pause markers, pace scales length, measured pitch per voice and per semitone, hostile scripts; engine: narration placed at the playhead with clip length = speech (sample-exact), never overwrites (busy tracks → next free track → new track), occupied/locked/unknown track refused with no change, edit keeps start/length/speed (longer speech: whole speech in the file; shorter: file padded with exact zeros), edits write beside the previous file, undo/redo restore the project exactly, enabled only with a narration clip (or `clip`), 16 hostile parameter sets refused without change, preview changes nothing, save/reload keeps narrations, the narration is audible in `mix_sequence`; UI: type + Add pause + Add to timeline, the new clip is selected and loaded, clicking the clip on the timeline loads its script, Save edits it in place as one undo step, Advanced twirl, panel opens beside Properties; natural voices: 50-sentence Whisper-base WER 2.1–3.6 % for all 9 voices (crate README), loaders fuzzed with hostile files, `tts.render` cache, download refused without the feature |
| Remix | `crates/audio-dsp/src/remix.rs`, `crates/render/src/remix.rs`, `crates/engine/src/remix_tests.rs`, `crates/ui-egui/tests/it/remix_ui.rs` | generated rhythmic music (Rust-synthesised drums, bass, chord sections at 90–128 BPM, 22.05/44.1/48 kHz): tempo within 0.3 BPM, every beat within 10 ms (measured ≤ 8.9 ms); remix targets from 0.4× to 2.2× the source within one beat (measured ≤ 12 ms at default sliders), every cut on a detected beat, intro and outro kept, pieces ≥ 4 beats, deterministic; silence, tone and noise refused; rendering independent of request cuts, source-exact outside the 20 ms equal-power crossfades, no clicks; the engine's mix equals the source pieces (< 1e-5); undo, redo and revert exact; overlaps refused; Remix Properties dialog and Remix tool drag driven by automation id |
| Oracle | `crates/*/tests/{,it/}*oracle*.rs`, `conformance.rs` | compare with ffmpeg/ffprobe (§2) |
| Robustness / fuzz | `*/tests/{,it/}robustness.rs`, `*/tests/{,it/}fuzz.rs` | seeded mutation and truncation of real and synthetic files; nothing may panic |
| Program monitor while dragging | `crates/ui-egui/tests/it/perf_ui.rs` | headless: a parameter changed on every UI pass (a value dragged in Effect Controls or Properties) and a backward scrub leave the Program monitor's picture at most one step behind (it shows the frame asked for the pass before while the frame due renders; it used to freeze until the mouse rested); a frame still rendering is kept when the same frame is asked for at a newer revision, so frames slower than a refresh (CPU effects) are shown too; `perf.stats` reports the picture's frame and revision against the playhead and the project |
| Performance | `*/tests/{,it/}perf.rs` (`#[ignore]`) | §5 |

Commit `*.proptest-regressions` files so failing cases are re-run.

## 2. ffmpeg oracle tests

ffmpeg and ffprobe are **external processes** used to generate fixtures and to check results. They
are never linked or shipped ([AGENTS.md](../AGENTS.md) §2).

- The tools are found by `filmcraft-testkit` (a dev-dependency-only crate, `crates/testkit`), in
  this order: `FILMCRAFT_FFMPEG` / `FILMCRAFT_FFPROBE` (explicit paths; a wrong path is an error),
  then every directory on `PATH` (`ffmpeg` and `ffmpeg.exe`), then well-known install directories
  (`/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`, `C:\ffmpeg\bin`, …).
- If a tool is missing the test prints `SKIPPED (<test>): ffmpeg not found …` and passes. Set
  **`FILMCRAFT_REQUIRE_ORACLES=1`** (CI) to make every such skip a failure. In new tests use
  `let ff = filmcraft_testkit::require_ffmpeg!();` (also `require_ffprobe!`, `require_oracles!`).
- Some oracles need a recent ffmpeg: the APV tests (`crates/apv/tests/it/oracle.rs`) skip when
  `ffmpeg -formats` does not list `apv` (ffmpeg ≥ 8.0), naming the detected version.
- Fixtures are generated on first use into `<workspace>/target/fixtures/<crate>/`
  (`filmcraft_testkit::fixtures_dir`; independent of `CARGO_TARGET_DIR`, so agents with private
  target dirs share them; `FILMCRAFT_FIXTURES_DIR` overrides the root) and reused afterwards.
  Generators write to a per-process/thread temporary name (`testkit::temp_path`) and rename it into
  place, so concurrent tests never see half-written files. Delete the directory to regenerate.
  Never commit media.
- Every crate's `tests/common/mod.rs` delegates discovery and fixture paths to testkit, except
  `crates/codecs/src/tests.rs`, which still has its own lookup (the crate was being edited
  concurrently; migrate it when convenient).
- `cargo xtask fixtures [crate…]` pre-generates the whole fixture matrix up front (useful before a
  parallel test run or on a fresh machine). It runs each crate's ignored `generate_fixtures` test —
  the same generators the oracle tests call — for `h264`, `hevc`, `isobmff`, `matroska`, `dnx`,
  `prores`, `mpeg2v`, `ac3` and `codecs` (MXF, Ogg and MPEG TS / PS), and prints one `made` / `cached` / `skipped` line per fixture plus a summary. The
  `aac`, `opus` and `h264enc` oracles generate small per-test signals on demand and are not part of
  the matrix.
- Fixture sources are synthetic: `testsrc2`, `mandelbrot`, SMPTE bars, noise and fades, sine tones.
  H.264 and HEVC fixtures need ffmpeg built with libx264 and libx265. VideoToolbox fixtures are
  generated only on macOS. The Media Foundation parity tests (`crates/platform/tests/it/media_foundation.rs`,
  Windows only) make their 640x360, 1080p and 2160p H.264 / HEVC / Main 10 fixtures with ffmpeg and skip
  without a Direct3D 11 video device or the HEVC / VP9 / AV1 codec extensions of the Microsoft Store. The VP9 and
  AV1 fixtures (libvpx-vp9, libaom-av1; 360p, 1080p, 2160p, hidden alt-ref frames, two GOPs) need an ffmpeg with those encoders.

The NVENC tests (`crates/platform/tests/it/nvenc.rs` and `nvenc_export.rs` for H.264, `nvenc_rgba_input.rs` for
the GPU's RGB → 4:2:0 conversion, `nvenc_hevc.rs`,
`nvenc_hevc_export.rs`, `nvenc_hevc_probe.rs` and `nvenc_hevc_warm.rs` for H.265, `nvenc_hevc_main10.rs` and
`nvenc_hevc_hdr_export.rs` for Main 10 HDR; Windows only) skip, printing `SKIPPED`, without an NVIDIA GPU
with NVENC (H.265: when the HEVC probe says there is no HEVC encoder; Main 10: when the Main 10 probe says
there is no 10-bit encoder; on a machine that has one, a configuration NVENC refuses fails the test). The FFI layout tests in
`crates/platform/src/nvenc/abi_tests.rs` were generated from a C program built with MSVC
(`cl` after `vcvars64.bat`) against NVIDIA's MIT-licensed `nvEncodeAPI.h` (12.1); the header and the
program are not in the repository. To regenerate them, put the header in a scratch directory and
write a program that prints, for every type, field, constant and GUID of `src/nvenc/ffi.rs` that the
asserts name (H.264, HEVC and the HEVC picture parameters with their SEI payload array), `sizeof` / `alignof` / `offsetof`, the enum and macro values cast to
64-bit integers and the GUIDs as 128-bit hex; for the bit-fields of the `flags` words, which
`offsetof` cannot take, zero a structure, set that one field to its maximum and print the 32-bit word.
Compare the numbers with the asserts (a difference is drift, which must be understood before
anything is "fixed" to match) and add asserts for anything new.

### Pass criteria per codec

| Crate | Oracle check | Criterion |
|---|---|---|
| `h264` | decode every fixture single-threaded and frame-threaded; compare with `ffmpeg -f rawvideo -pix_fmt yuv420p` | **bit-exact**, every frame |
| `hevc` | same, `yuv420p` / `yuv420p10le` | **bit-exact** (8- and 10-bit) |
| `h264enc` | `ffmpeg -v warning -err_detect +crccheck+bitstream+buffer+explode` decodes our stream | no output at all (a `corrupt decoded frame` warning fails; a negative control checks dropped and truncated slices are reported), exact frame count, **bit-identical to the encoder's reconstruction**; B-frame order checked with ffprobe; rate-control targets |
| `prores` decode | ffmpeg `prores_ks` / `prores_aw` fixtures, compared at native depth | within **±1 LSB** (different integer IDCT); alpha bit-exact |
| `prores` encode | ffmpeg decodes with `-xerror` | no errors; agrees with our decoder within ±1 |
| `dnx` decode | ffmpeg VC-3 fixtures (13 DNxHD CIDs, DNxHR LB/SQ/HQ/HQX/444), compared at native depth | within **±2 LSB**, ≤ 100 samples per million beyond ±1 (ffmpeg's integer IDCT; we evaluate the exact IDCT) |
| `dnx` encode | ffmpeg decodes with `-xerror` | no errors; agrees with our decoder within ±2; HQ luma PSNR ≥ 45 dB |
| `aac` decode | ffmpeg's decode of the same stream | max abs error ~1e-7 (float) |
| `aac` encode | ffmpeg decodes our stream | no errors, no clipping, CBR within ±5% of target; SNR reported |
| `isobmff` | `ffprobe -show_packets` on ffmpeg-made MP4/MOV | packet offsets, sizes, pts/dts, durations, key flags and stream parameters equal; remuxed files decode with `ffmpeg -v error` silent |
| `matroska` | `ffprobe -show_packets` | every packet (stream, size, key flag, pts, duration) equal; seeks land on the latest keyframe ≤ target |
| `audio-dsp` loudness | `tests/loudness_oracle.rs`: signals generated in Rust (997 Hz sine, pink noise at 48/96 kHz, speech-like bursts at 48/44.1 kHz, stereo with silence and sub-gate passages, an fs/4 inter-sample-peak tone) written as float WAV and measured with `ffmpeg -af ebur128=peak=true:metadata=1` | momentary and short-term every 100 ms **±0.1 LU**, integrated **±0.1 LU**, LRA **±0.5 LU**, true peak **±0.2 dB** (against the analytic value when one exists). Measured: ΔM/ΔS ≤ 0.0005 LU, ΔI ≤ 0.007 LU, ΔLRA ≤ 0.04 LU, ΔTP ≤ 0.045 dB against ffmpeg. On the fs/4 tone ffmpeg's own true peak is +0.6 dB high (−0.32 vs the analytic −0.92 dBTP); ours is −0.05 dB |
| `audio-dsp` effects | `src/effects/premiere_tests.rs`, `src/design.rs`, registry tests in `src/effects/mod.rs`: generated tones, noise and impulses | every effect: neutral settings = delayed identity (≤ 2·10⁻⁴), bit-exact determinism, latency constant and equal to where an impulse lands, block-size independence, finite / denormal-free tails at extreme settings, > 1× realtime; per effect a level or frequency-response check (measured tone vs analytic `response_db` within 0.1–0.2 dB, Butterworth vs closed form 10⁻⁶ dB, Chebyshev/elliptic ripple and stop-band bounds, LR4 crossover sum flat within 0.02 dB, comb-notch depth, echo positions to the sample, Schroeder RT60 of generated impulses within 25 %, click repair −20 dB) |
| `mxf` (via `codecs/tests/mxf_oracle.rs`) | ffmpeg-written OP1a / OP-Atom / D-10 files decoded by ffmpeg | H.264 (long GOP, B pictures without temporal offsets) **bit-exact** every frame and at 25 random seeks; DNxHR ±2, ProRes ±1; PCM / AES3 **sample-exact**; frame count = ffprobe packets; timecode (25 fps, 29.97 DF); MPEG-2 long GOP, XDCAM HD422 (1080i 4:2:2) and D-10 / IMX (720×608 4:2:2) within ±4 (`mpeg2v` criterion); truncation/mutation never panics |
| `mpeg2v` | ffmpeg-encoded MPEG-1/2 elementary streams (progressive, TFF/BFF interlaced, 4:2:2 intra and long GOP, 1080i, escapes + custom matrices, MPEG-1) and our synthetic streams with field pictures / 16x8 / dual prime / concealment vectors, decoded by ffmpeg (`-xerror`) | per frame max ±4 and PSNR ≥ 58 dB, ≤ 3 % of samples differing (ffmpeg's integer IDCT vs our IEEE 1180-accurate one; measured max ±1-3, 65-75 dB); frame count, display order, picture types, interlacing and field order **exact** (ffprobe); IEEE 1180 procedure passes |
| MPEG TS / PS (via `codecs/tests/mpeg_oracle.rs`) | ffmpeg-written TS (MPEG-2 + MP2, H.264 + AAC, HEVC + LATM AAC), BDAV `.m2ts` / `.mts` (H.264 + Blu-ray LPCM / AC-3), VOB (MPEG-2 + MP2 / DVD LPCM 16 and 24-bit / AC-3), MPEG-1 system streams, `.m2v`; MPEG-2 in MOV (XDCAM `xd5c`), MP4 (`mp4v`), MKV | frame count and every frame's **PTS exact** (ffprobe); H.264 / HEVC **bit-exact**, MPEG-2 within the `mpeg2v` criterion; random seeks = sequential decode; LPCM **sample-exact**; AAC ≤ 2.1·10⁻⁷, MP2 (symphonia) ≤ 3.2·10⁻⁵, AC-3 ≤ 5·10⁻³ (decoder-specific dither; measured 2.3·10⁻³); random-access audio = sequential |
| `ac3` | ffmpeg AC-3 encodes (2/0 with coupling and rematrixing, 1/0 32 kHz, 3/2+LFE 448 kb/s, noise) decoded by ffmpeg | SNR per channel ≥ 40-60 dB on tonal content (measured 66-94 dB), ≥ 18 dB on noise (27.7 dB, the same as between two of our own dither seeds) |
| `ogg` (via `codecs/tests/ogg_oracle.rs`) | ffmpeg (libopus, Vorbis) files decoded by libopus / ffmpeg | length **exact** (pre-skip, end trimming); SNR ≥ 50 dB CELT (measured 77.7), ≥ 10 dB SILK, Vorbis 139 dB; random seeks within 6·10⁻⁶ of the continuous decode |
| MP4 audio at another rate (`codecs/tests/mp4_oracle.rs`) | ffmpeg-written AAC in MP4 (48 kHz, edit list skipping the priming) | reads resampled to 44.1 kHz in frame-sized requests equal the native decode interpolated at the same times (difference < 10⁻⁴, peak ≤ the source level) |
| image sequences / BWF (`codecs/tests/media_oracle.rs`) | ffmpeg-written PNG/TIFF/BMP/JPEG stills read by ffmpeg's image2 demuxer; ffmpeg BWF (`-write_bext`) | PNG/TIFF/BMP **exact**, JPEG ±12 (measured 3); missing frames hold the previous one; BWF TimeReference = ffprobe's, start timecode, samples exact |
| `export` | our own demuxer/decoder reads the file back; ffprobe counts frames when present | expected size, duration, colour, audio level; exact frame count |
| `export` settings and presets (M6.5) | `export/src/settings_tests.rs`, engine `export_tests.rs`: every built-in preset exports a slice of the demo sequence; ffprobe reads codec, profile, size, rate, audio format and data rate; our decoders read pixels back | codec / profile / container / size / `r_frame_rate` / sample format exact; H.264 bitrate ≤ VBR max × 1.1 and ≥ ½ target over 3 s; ProRes ≤ 1.5 × nominal; DNxHR within 25 % of nominal; loudness normalization within **0.5 LU** of the target (WAV and AAC), true peak ≤ ceiling + 0.1 dB; image sequences numbered `<name>000…`; limiter, overlays, metadata (`title`/`copyright` tags), MOV multiplexer, two-pass/CBR progress |

Each codec README has the full fixture matrix and the measured results.

## 3. Rendering: CPU reference and GPU parity

- The CPU compositor in `filmcraft-render` is the reference. Its tests check computed values
  (opacity blends, dissolve midpoints, Motion placement, ½-resolution against downsampled full
  resolution) and that `render::plan` matches `render_sequence`.
- `crates/gpu/src/tests.rs` composites the same frame plan (a YUV layer plus a transformed,
  semi-transparent RGBA layer) on the GPU and on the CPU. It requires a 99th-percentile channel
  difference of ≤ 6 and a mean of < 1.5 (8-bit levels). It skips when no GPU adapter is available.
- Effects implemented only on the CPU need no GPU test: the plan pre-renders those layers on the
  CPU.
- **Golden images** (`crates/golden`, a test-only crate): `tests/golden.rs` builds small procedural
  projects (demo-generator footage, bars, no media files) at 320×180 and renders one frame of each
  through the CPU compositor: Motion transform + opacity, four blend modes (Multiply, Screen,
  Overlay, Difference), Gaussian Blur, Lumetri basic correction, Crop, Cross Dissolve at 50 %, Dip
  to Black at 25 %, Wipe at 50 %, Timecode / Clip Name burn-in text, graphic clips (also a built-in
  graphics template with overridden properties and per-character styles with a pinned box), and colour
  management: S-Log3/S-Gamut3.Cine footage interpreted into Rec. 709 (`log_to_rec709`) and
  Rec. 2100 PQ footage tone mapped into Rec. 709 (`hdr_tone_map`; the footage is demo frames
  re-encoded by the test's `Encoded` source). Each frame is compared
  with `crates/golden/goldens/<scene>.png` by `filmcraft_testkit::golden`:
  **PSNR ≥ 45 dB, max abs ≤ 12, 99th-percentile per-pixel max channel difference ≤ 2** (8-bit
  sRGB levels; `Tolerance::RENDER`). On failure the actual frame and a ×8 difference image go to
  `<workspace>/target/golden-failures/`. Six `vfx_*` scenes cover the M5.11 effects (Corner Pin over Turbulent
  Displace, Ultra Key, Wonder Glow + Glint, Spacer + Stroke + Long Shadow over Gradient, VR Rotate Sphere,
  Channel Mix + Color Emboss + Rounded Crop); per-effect unit tests live in `crates/render/src/vfx/tests.rs`
  (identity at neutral parameters, determinism, range, a known value each, temporal/track effects through
  the compositor). `scenes_are_distinct` guards against blank or duplicate
  scenes.
- **Video transitions** (`crates/render/src/transitions/tests.rs`) are checked for every transition
  (84 modern, 21 Legacy, 7 hidden Obsolete): p = 0 is exactly the outgoing frame and p = 1 exactly
  the incoming one, near both ends the output is close to that frame (continuity), the midpoint differs
  from both, the output is finite and deterministic (also on 1×1 / odd sizes and NaN progress), every
  parameter visibly changes the picture, and every direction differs. Golden *fingerprints* (4×4 mean
  RGB grid at p = 0.25/0.5/0.75, ±4 levels) live in `src/transitions/goldens.txt`; re-bless with
  `FILMCRAFT_BLESS=1 cargo test -p filmcraft-render golden_fingerprints` and review the diff.
  `FILMCRAFT_TRANSITION_SHEETS=<dir> cargo test -p filmcraft-render contact_sheets -- --ignored`
  writes one contact-sheet PNG per folder for eyeballing (never commit them).
- **Blessing:** `FILMCRAFT_BLESS=1 cargo test -p filmcraft-golden` rewrites the references (and
  writes a `.attribution` sidecar for any new one; it prints the `ATTRIBUTION.md` row to add).
  References are original work rendered by FilmCraft, must stay under 50 KB (enforced on bless),
  and every one needs its sidecar and index row (`cargo xtask assets`). Look at a re-blessed PNG
  before committing it.
- The same scenes run through `plan_frame` + `GpuCompositor` when a GPU adapter exists
  (`gpu_matches_cpu_on_golden_scenes`, same p99 ≤ 6 / mean < 1.5 criterion; skipped otherwise).
  This found a real bug: the GPU upload cache was keyed by pixel-buffer address without keeping
  the buffer alive, so a new frame allocated at a freed frame's address was drawn with the stale
  texture (fixed; regression test `upload_cache_keeps_buffers_alive`).

## 4. UI: control channel and screenshots

### Scripted UI tests (headless)

`crates/ui-egui/tests/it/scripted.rs` runs the real `FilmcraftApp` under
[`egui_kittest`](https://docs.rs/egui_kittest) (`build_eframe`): no window, no GPU, no OS event loop.
A small `Driver` opens the demo project, sends requests through the same control channel agents use
(`ControlRequest` → `control::handle`, exactly as the TCP server does) and steps egui frames until
each reply arrives. Synthetic input queued by `ui.click` / `ui.drag` / `ui.key` is moved into the
next frame by the app's own `raw_input_hook`, so clicks by automation id work headless. Covered:

| Test | Asserts |
|---|---|
| `demo_project_opens_headless_and_registers_widgets` | active sequence, > 50 registered widgets, panel and tool ids, 6 clips on V1 (`sequence.inspect`) |
| `razor_then_undo_through_the_control_channel` | `timeline.razor` adds a clip, the timeline can locate it on screen, `edit.undo` via `ui.menu.invoke`, redo |
| `insert_from_source_ripples_the_sequence` | `source.open` + marks + `source.insert` lengthens the sequence |
| `apply_effect_appears_in_effect_controls` | `effects.apply` adds Gaussian Blur in the model and `effectControls.effect.gaussian_blur` appears in the panel; undo removes it |
| `playback_toggle_and_stop` | `playback.toggle` / `ui.playback stop` and `ui.inspect` playback state |
| `clicking_a_tool_button_by_automation_id` | `ui.click {id: "tools.Razor"}` changes the tool (real egui input path) |
| `unknown_methods_and_commands_fail_cleanly` | errors come back as `{"ok": false}` |

`crates/ui-egui/tests/it/mixer_ui.rs` drives the Audio Track Mixer, Audio Clip Mixer, timeline track
keyframes and the Audio Gain dialog by automation id and with multi-frame pointer drags (press, move
over several frames, release). With `FILMCRAFT_UI_SNAPSHOT_DIR=<dir>` it also renders the window
offscreen through wgpu (`Harness::render`) and writes `mixer-*.png`; this works without a visible
window (for example on a locked screen, where `ui.screenshot` cannot capture).
`crates/ui-egui/tests/it/essential_sound_ui.rs` does the same for the Essential Sound panel (type buttons,
switches, a slider drag as one undo step, section bypass, Auto-Match, ducking, Browse presets;
`essential-sound-*.png`).
`crates/ui-egui/tests/it/clip_audio_ui.rs` covers the audio of video clips (#223): the linked audio's
Volume, Channel Volume and Panner in Effect Controls and Properties, and the Volume line on audio
clips in the Timeline (a drag is one undo step, Pen-tool keyframes, keyframe drags, a click or
right-click on the line still reaches the clip; `FILMCRAFT_UI_SHOTS=<dir>` writes `volume-*.png`).

Essential Sound engine tests (`crates/engine/src/essential_sound_tests.rs`) build projects from
generated speech-like and tonal WAVs: Auto-Match lands within ±0.5 LU of the target (measured: 0.000 LU,
with and without a repair/clarity chain) and measures a mono clip as one channel (3.01 LU below the
same signal as dual mono, within 0.1 LU of a one-channel meter over the file); ducking on a dialogue +
music project gives keyframes within 60 ms of the expected times (measured 20 ms) and −15.00 dB in the
mix; each repair stage improves its metric through the render path (hum −39 dB, rumble −22 dB, noise
floor −13 dB, sibilance −19 dB, reverb tail −10 dB); the mix is bit-identical however requests are cut
and the WAV export equals it.
`perf_full_dialogue_chain_realtime_factor` (ignored; run with `--release`) prints the realtime factor of
all nine Dialogue effects on one clip (22× on one core).

`crates/ui-egui/tests/it/export_ui.rs` drives Export mode: the settings column and Summary, editing a
setting turning the preset into Custom, the Preset Manager (search, favourite star, save, apply,
delete), the queue panel (Send to Queue, Up/Down, Cancel, Start, Retry, Clear) with real encodes,
the Export button and the header's Quick Export popup (`export-*.png` snapshots).

`crates/ui-egui/tests/it/color_ui.rs` drives colour features the same way (`color-*.png` snapshots):
Lumetri Input/Look LUT menus and section switches, the Interpret Footage ▸ Color Management and
Sequence Color Management dialogs, and the HDR scopes of a PQ sequence.

Colour science tests: `filmcraft-color` (curve round trips and published reference values per
camera log curve, BT.709/BT.2087 matrices, BT.2390 EETF, gamut mapping, LUT tetrahedral vs a
brute-force barycentric reference, `.cube`/`.3dl` round trips), `filmcraft-gpu`
(`gpu_lut_matches_cpu_tetrahedral`: WGSL vs CPU, max |Δ| ≈ 2e-7), `render::colorman`,
`render::color_match`, engine `color_tests.rs`, and `export::tests::hdr_exports_signal_pq_and_hlg`
(PQ/HLG exports read back by our demuxer and by ffprobe: `color_transfer`, `color_primaries`,
`color_space`, mastering display and content light side data).

Not covered headless: `ui.screenshot` (needs a real viewport), the wgpu monitor path (the harness
runs the CPU texture path), audio output, and wall-clock playback advance (kittest frames do not
advance real time). Playing sound is covered without a device: `play_ahead` tests drive the
mixed-ahead ring with a wall-clock device (order, underruns stay in time, mixer panic, stop), and
`cargo run --release -p filmcraft-ui-egui --example bench_audio -- <project.fcproj>` plays a real
project's sequences through a simulated 48 kHz device and counts dropouts, mixing in the callback
(`direct`, as before) against mixing ahead (`ahead`). Use `Driver` for new UI regressions: `d.exec(command, params)`,
`d.ok(method, params)`, `d.frames(n)`.

### Interactive checks

For visual work, drive the real app:

```sh
cargo run --release -p filmcraft -- --control 9876
```

Then script it over the control channel or MCP: run commands, click by automation id,
`ui.inspect` / `ui.elements` to assert state, and `ui.screenshot {"path": "…"}` to capture the window
or a single panel (`{"panel": "Timeline"}`). Look at every screenshot. Examples are in
[agents.md](agents.md). The headless path (`filmcraft-cli run script.jsonl`, MCP `--demo`) covers
engine behaviour without a window.

## 5. Performance

| Benchmark | Command |
|---|---|
| H.264 decode, 1080p, threads sweep | `cargo test --release -p filmcraft-h264 --test it perf:: -- --ignored --nocapture` |
| HEVC decode, 1080p / 2160p | `cargo test --release -p filmcraft-hevc --test it perf:: -- --ignored --nocapture` |
| ProRes decode/encode | `cargo test --release -p filmcraft-prores --test it perf:: -- --ignored --nocapture` |
| Encoder speed / PSNR / bitrate | `cargo run --release -p filmcraft-h264enc --example h264enc_synth -- …` |
| Export to H.264 + AAC on the CPU (1080p clip through `file.exportMedia`, wall time per run, `--keep out.mp4` to compare files) | `cargo run --release -p filmcraft-engine --example bench_export -- --runs 7` |
| Decode a real file through the media stack | `cargo run --release -p filmcraft-cli -- bench-decode file.mp4 --frames 120` |
| Coding-tool coverage of the fixtures | `cargo test --release -p filmcraft-h264 --test it conformance::coverage_report -- --ignored --nocapture` (same for `hevc`) |
| **Whole-app suite**: decode fps per codec, playback, scrubbing, timeline UI, export, project save/open, peak RSS | `cargo xtask bench` ([performance.md](performance.md)) |

`cargo xtask bench` (the example `crates/ui-egui/examples/bench/`) runs six sections, each in its
own process under `/usr/bin/time` so peak RSS is per section: `decode` (every frame of 1080p and
2160p H.264, HEVC, VP9, AV1 and ProRes through the media stack), `playback` (the scenarios below),
`scrub` (random seeks and playhead drags: time until the exact frame is on screen), `timeline`
(the real app under `egui_kittest` with a 1000-clip / 20-track sequence: update, tessellation and
wgpu render ms per frame), `export` (H.264 + AAC and ProRes) and `project` (save / open 5000 clips).
Options: `--sections decode,scrub`, `--only <substring>` (fixture or scenario), `--repeat N`,
`--quick`, `--cpu`, `--hw auto|off` (Settings ▸ Playback ▸ Hardware decoding; default `auto`, as the app), `--label NAME` (output `target/bench/bench-<label>.{json,md}`),
`--section NAME --json FILE` (one section in-process, e.g. under a profiler). Fixtures are made
with ffmpeg in `target/fixtures/playback/` (or `$FILMCRAFT_FIXTURES/playback`). The suite's
`playback` section plays `h264-1080`, `stack3` and `stack3-blend` at Full, `h264-2160` at Full, 1/2 and 1/4 and with
draft decoding (Settings ▸ Playback ▸ Draft decoding) at 1/2 and 1/4, and `hevc-2160`, `vp9-2160`
and `av1-2160` at Full, 1/2 and 1/2 draft; its `draft` column counts the frames decoded in draft
mode.

The perf tests check bit-exactness before they time anything. Results go in the crate README's
performance table, with machine and thread count. Headline numbers go in
[ROADMAP.md](../ROADMAP.md). Measure on an idle machine: parallel agent builds distort timings.

### Playback benchmark

`cargo xtask bench-playback` (the example `crates/ui-egui/examples/bench_playback.rs`) plays
sequences headlessly through the Program monitor's own frame scheduler: the `FrameServer` worker
pool, `schedule_playback` (prefetch order and stale-job dropping), its caches, and the
`PlaybackMeter` that counts shown/dropped frames in the app. On the GPU path it also composites
each plan with `filmcraft-gpu`, as the monitor does on the UI thread.

```sh
cargo xtask bench-playback                                   # every scenario, GPU path, Full and Half
cargo xtask bench-playback --scenario stack3 --res full --cpu
cargo xtask bench-playback --json target/bench-playback.json # machine-readable results
```

| Scenario | What plays |
|---|---|
| `h264-1080` | one 1080p23.976 H.264 clip (testsrc2 + grain, ~45 Mbit/s, 250-frame GOP) |
| `stack3` | three 1080p H.264 clips on V1–V3; V2/V3 scaled, positioned, rotated, 70–85 % opacity |
| `stack3-blend` | `stack3` with V2 in Screen and V3 in Overlay (blend modes on the GPU) |
| `h264-2160` | one 2160p23.976 H.264 clip |
| `demo` | the built-in demo project (procedural footage, transitions, effects) |
| `after-preview` | 1080p H.264 + Lumetri/Sharpen/Levels/Tint: a live play, Render Effects In to Out, then two plays of the green segment |
| `seek-storm` | 40 jumps to random frames 150 ms apart (scrubbing), time until the exact frame shows |

Options: `--res full,half,quarter`, `--cpu` (CPU compositor + texture conversion instead of the
GPU path), `--seconds`, `--refresh` (display Hz), `--workers`, `--repeat`, `--json <file>`.
Fixtures are made with ffmpeg in `target/fixtures/playback/` (set `FILMCRAFT_FIXTURES` to share one
set between worktrees).

Columns: **shown/drop** as counted in the app (a frame is shown when its exact picture was on
screen at a refresh while it was due; frames passed over without a refresh count as dropped);
**ontime** = due frames whose job finished before they were due; **lat** = queue→ready per job,
**svc** = worker time per job; **cpu/j**, **src/j**, **srcC/j** = worker thread CPU, source fetch
(decode) wall and thread CPU per job; **ui** = time on the UI thread to present a frame (GPU upload
and draw, or texture conversion); **cpu ms/f** = process CPU per frame and **cores** = the cores that
needs at the sequence frame rate; **seeks/dec** = decoder restarts and samples decoded; **waste** =
jobs for frames that were never due.

Wall-clock columns (shown/drop, ontime, latencies) depend on machine load, so each row prints the
load average. CPU columns (thread and process CPU time) and the structural counters (seeks,
samples decoded, wasted jobs) do not, and are what to compare between runs on a busy machine.

Results on an M4 Pro (14 cores), GPU path, 8 s plays, median of 2 alternating runs of the M4.6
baseline (commit `5170376`) and the result of M4.6, on a machine shared with parallel agent
builds (**load average 86–207**, so wall-clock columns are pessimistic; CPU ms/frame is not):

| Scenario | shown/dropped before | after | CPU ms/frame before → after | decoder seeks |
|---|---|---|---|---|
| h264-1080 Full / Half | 106/86, 118/74 | **192/0, 192/0** | 80 → 46, 95 → 46 | 3–4 → 1 |
| stack3 (3 × 1080p) Full / Half | 185/7, 174/18 | **192/0, 192/0** | 136 → 109, 117 → 111 | 4 → 3 (one per source) |
| h264-2160 Full / Half | 0/192, 0/192 | 5/187, 19/173 | 110 → 135, 170 → 145 | 4–5 → 1–2 |
| demo Full / Half | 63/129, 124/68 | 97/95, 152/40 | 140 → 80, 93 → 40 | |
| after-preview, 1st play after render (Full) | 80/112 | **190/2** | 119 → 26 | 104 → 0 |
| after-preview, 2nd play (Full) | 145/47 | **192/0** | 76 → 26 | 132 → 0 |
| after-preview, live effects (Half) | 18/174 | 69/123 (frames skipped evenly) | 109 → 95 | |
| seek-storm (40 jumps, 150 ms each) | 0/40 shown | 6/40 | 827 → 666 per jump | |

At load ~25–60 the same final build plays h264-1080, stack3 and h264-2160 at Full with 0 dropped
in 3 of 3 runs (192/0) and render previews 192/0. The 4K fixture needs ~4 cores of decode per
real-time second (≈160 ms CPU per frame at 170 Mbit/s); the demo project's procedural footage
~190 ms per Full-resolution frame.

### Audio mixer throughput

`perf_24_tracks_3_effects_realtime_factor` (24 tracks, 72 inserts and a compressed submix on one
core) must run at least 4x realtime in release builds (1x in debug). On Unix it times the test
thread's CPU time, so it runs with the rest of the suite. Hosts without a per-thread CPU clock
(Windows) fall back to wall time, which depends on load, so there it is ignored by default; run it
explicitly with
`cargo test --release -p filmcraft-render --lib perf_24_tracks_3_effects_realtime_factor -- --ignored --nocapture`
and record the machine with the result.

## Local Codex console qualification

`cargo test -p filmcraft codex_console` tests bounded protocol parsing, isolation policy, turn
startup/cancellation races, scoped direct-command approval, rapid reconnect and output limits. The ignored
`installed_app_server_discovers_real_filmcraft_mcp` test requires `FILMCRAFT_CONSOLE_TEST_CLI`
pointing to a built CLI and `codex` on PATH. Run it with `-- --ignored --nocapture`; it uses an
isolated temporary Codex home, includes a conflicting required MCP server, and sends no model
prompt. Never point synthetic test configuration at the user's Codex home.

`cargo test -p filmcraft-ui-egui --lib console::` verifies native command discovery, nonzero playhead/
selection, direct edits, atomic batch Undo/Redo and rollback, hostile parameters, overwrite refusal,
stale analysis IDs and owned passage trim. Legacy preview tests retain stale/expired proposal coverage.
`cargo test -p filmcraft-ui-egui --test command_console_ui` drives real widgets with a deterministic
adapter, covering Send, streamed text, direct edits, Undo, failures, Cancel and session controls.
Routine edits in `filmcraft-console/3` need no Apply/Reject; separate controller profiles retain their
own preview/commit contracts. Set `FILMCRAFT_CONSOLE_SCREENSHOT` for an offscreen render. Native
attended testing requires user authorization and the shared GUI lease. Use a separate development
executable, disposable project and isolated data directory; close the owned executable before
releasing the lease. Never replace the everyday installation or modify the original project.

`cargo test -p filmcraft-automation bridge::` includes the original `bridge::tests` and the ten new
`bridge::fault_tests` cases in `src/bridge_fault_tests.rs`. They cover apply-then-lost-reply, uncertain
writes/read timeouts and malformed replies without a second edit/job. A valid
`{"ok":false,"error":"timeout"}` UI-reply timeout is also uncertain: a deferred-apply regression proves
the original queued edit can execute after that error. Other valid app rejections remain app errors.
Only the exact retry-safe allowlist may retry, not arbitrary commands labelled read-only; even
`console.bridge.memory` reads can synchronize project state. Replies must be newline-complete with
a matching ID and boolean `ok`; omitted successful `result` remains compatible as null.
The focused automation run passed 53 tests (43 original plus 10 new fault cases), with no failures.
This is not a new combined desktop CI result; the integration coordinator owns the full combined
`cargo xtask ci` run and final integration commit.

The model picker uses the account's actual `model/list` catalog. Tests cover pagination, hidden
models, unavailable choices, per-model effort defaults and restoring the original default model.
Session control commands have hostile-parameter tests and use the same functions as UI widgets.

`codex_voice_round_trip_uses_generated_speech_without_devices` is an ignored provider test. Set
`FILMCRAFT_CONSOLE_TEST_CLI` and `FILMCRAFT_VOICE_TEST_PCM` (bounded 8 kHz mono PCM16LE generated
speech). It uses the existing Codex sign-in and a read-only synthetic demo, opens no audio devices,
and verifies actual WebRTC speech transmission and returned audio. Set
`FILMCRAFT_VOICE_TEST_PROJECT=1` with a generated project-inspection utterance to require a completed
Codex/MCP inspection handoff. Never use ambient/private microphone audio in this fixture test.
G.711 coverage exhausts the byte-sized decode domain, tests endpoint vectors and hostile floats;
jitter tests cover out-of-order, duplicate, wrap and oversized packets. UI tests exercise Voice
On/Off and stopping on console close with deterministic hooks. Actual microphone use is an
explicit user action and requires the macOS bundle privacy description and OS permission.


Project-memory/session regression coverage (`console::sessions::tests`) uses temporary projects and
files, never the user's project or global Codex configuration. It checks restart/history restoration,
current external memory loading, model retention, voice off and no replayed pending operations,
unsaved conversation persistence, first-save memory migration, archive identifiers, corrupt/truncated/mutated and oversized files, concurrent index writers,
external memory edits and unwritable storage. Native protocol tests verify durable `thread/start`
and `thread/resume` rebuild the current isolated MCP transport, sandbox and memory policy. A regression test checks that fresh memory is supplied in a new
context turn on every resume, including memory that expands through JSON escaping.
Real-provider qualification must additionally prove a conversation remembers an earlier turn after
process restart, uses edited memory, and inspects the current FilmCraft project before continuing.

The opt-in compatibility test `codex_console::persistence_live::real_resume_keeps_conversation_and_refreshes_memory_after_pending_edit`
uses signed-in Codex plus a standalone read-only `filmcraft-cli mcp --demo`. It starts a durable
conversation, remembers a test phrase, leaves a marker proposal unapproved, kills its owned server,
resumes in a new server with changed memory, and verifies the phrase and new memory together after
fresh inspections. It opens no GUI or audio devices. Run with `FILMCRAFT_CONSOLE_TEST_CLI` pointing
to the built CLI, filtering the test and passing `-- --ignored --nocapture`. Regular CI skips it.

### Later local workflow evidence (2026-10-29)

The newer `filmcraft-console/3` run at `18ac73c8` supersedes the earlier review-ticket-only result.
Ignored local evidence in `target/command-console-workflow-20261029/` includes `qualification.json`,
`astra-review-packet.json` and `quality-checks.log`. Real Codex passed import and camera/studio
sequence placement, nonzero playhead/selection, direct grouped edits with one native Undo/Redo,
full-song performance candidate analysis, Beat Cuts/section markers (67 clips/278 markers), first-save
memory/thread continuity, reconnect and actual Resume-picker use after app restart. An eight-second
360×640/30 fps H.264/stereo 48 kHz AAC export completed and all 240 frames decoded. The original
project/everyday installation were unchanged; voice stayed off, the app closed and its lease released.
Recorded checkpoint gates passed: 3,987 tests, 49 skipped, strict lint, doctests, layers, 69 attributed
assets and WebAssembly for 49 crates. Later changes need fresh checks.

The user confirmed Codex voice worked in the later attended demo; the generated-speech test also
passed transcription, returned speech and same-thread MCP inspection without devices. These are
separate evidence from the voice-off editing run, not a requirement to repeat voice development.
`fullDemoReady` remains false: live match application/phrase/boundary/drift acceptance is unverified;
passage trim has only a synthetic regression; the export was not a full-song render; every registry
command was not individually exercised. Candidate scores do not establish the missing musical facts.

Before archival preserve these ignored reports, disposable project/media/export/screenshot copies,
`<project-path>.console/memory.md`, and the isolated application's conversation archive and unsaved
notes in `target/command-console-user-data/`. Codex stores full durable history separately; FilmCraft
caches only recent display messages. Treat context/media as private and never commit it or archive
live control tokens. Coordinate preservation with the owner of those runtime files.

### Consolidated main validation (2026-10-29)

The combined main source passed `cargo xtask ci`: formatting, strict workspace/all-target lint,
4,017 Nextest tests (49 explicitly skipped), doctests, layering, 69 attributed assets and the
WebAssembly check for 49 crates. The initial run exposed a next-free-ID error in the edit property
test fixture; its minimized seed is retained and the corrected 77-test edit suite passed. AV1's
injected inline panic fails without the new completion guard; the restored guard passes both the
threaded tests and 11 no-default-feature library tests. The benchmark example's two tests passed.

An owned generated-demo desktop session and actual console MCP subprocess passed native M/Add
Marker and Control+Command+M/Marquee behavior, the v3 tool contract, two edits in one Undo/Redo
action, and rejection preserving the saved project, revision, history and clean state. The
unobstructed editor/console screenshots were visually reviewed. Voice stayed off; no provider
conversation or everyday installation was changed. A regression also proves atomic batches reject
asynchronous `masks.track` before starting any tracking job; individual execution remains available.

Local evidence is under ignored `target/consolidation-20261029/`: `full-ci.log`,
`av1-before-guard.log`, `av1-inline-tests.log`, `bench-tests.log`, and `native/result.json` plus
screenshots. The full gate ran without external ffmpeg/ffprobe, so oracle cases that self-skip
inside a passing test are additional coverage limits beyond Nextest's 49 explicit skips. The later
benchmark fixture generator is a pinned, hashed external test-only binary in `oracle-tool/`; it is
never linked, bundled or shipped. This local gate is not live Windows/Linux or old-macOS evidence.

Required benchmark qualification then passed CPU H.264/AAC export with zero hardware/GPU frames
and a 5,000-clip persistent-Undo scenario. Empty scenarios, unavailable fixtures and invalid sections
all returned the expected nonzero status. `bench/qualification.json` records the development build
and machine-load limits; timings and sidecar size are summarized in [performance](performance.md).

### Console references and styles (2026-10-29)

The `filmcraft-console/4` source passed `cargo xtask ci`: formatting, strict workspace/all-target
lint, 4,032 Nextest tests (49 explicitly skipped), doctests, layering, 69 attributed assets and
the WebAssembly check for 49 crates. External-oracle cases that self-skip inside a passing test
remain additional coverage limits, as in the consolidated baseline; this is local macOS evidence.

The development workflow adds bounded text/PDF/PNG references and reusable creative defaults. Local real-Codex qualification read distinct text and PDF phrases, correctly
described an original PNG, received a selected preset, drafted a named style from agreed chat
preferences for review, saved it and loaded it in a fresh project/conversation. A controlled marker
edit followed the preset's Lavender default; an explicit Forest request took precedence. Native
Undo restored each edit. Project switching cleared pending references and re-established usable
editing context. No user project or physical microphone was used.

The native app also survived 24 truncated, bit-flipped, cyclic and deeply nested PDF cases while
retaining its generated project. Unit regressions cover file/preset budgets, corrupt inputs,
concurrent writers and an unheld lock left by a prior process, snapshot independence, Save As,
and voice-stop/context-refresh behavior across rapid project changes. Special-file rejection is
checked with a named pipe so reference reads do not wait for a writer.

Private evidence is retained under `target/console-next-20261029/`, including focused test logs,
full gate attempts and dated `native/` results/screenshots. These are local development checks,
not a cross-platform or every-format acceptance claim. See [formats and limitations](console-reference-inputs.md).
Actual audio listening remains unqualified: the isolated direct-PCM probe required API-key
authentication. Schema support, beat measurements and metadata references do not establish listening.


### Console overload and recovery repair (2026-10-30)

The attended 533-clip timeline exposed an inspection reply of 1,689,544 bytes, larger than the
old Console reader's 1 MiB frame limit. The separate later editor exit had no retained panic or
OS crash reason; do not describe it as proven fixed from the transport diagnosis alone.

Contract v5 uses compact, byte-budgeted native inspection pages, preserves large image-input
framing under a separate 16 MiB cap with a two-frame reader queue, and rejects stale page
revisions after edits. Tool result omission keeps the original error status and explicitly
forbids replaying edits to obtain a reply. Regression tests cover large timelines, every clip
identity across pages, hostile parameters/names, project byte-budget shortening, parameter and
keyframe pages, legacy Console inspection IDs, oversized results and credential redaction.

Recovery journals now carry revision-specific Undo/Redo history on the existing background
worker, independently of saved-project persistent Undo preference. Old journals without history
remain recoverable. Missing, corrupt, oversized, mismatched or wrong-generation history cannot
replace the recovered project. Previously lost history cannot be reconstructed from an old
snapshot alone. Journal/history writes remain separate transactions; an interrupted optional
history write must produce a warning while preserving recovery of the project itself.

A later two-process test on a private 2.5 MB complex-project copy confirmed exact serialized
project recovery and both Undo and Redo after SIGKILL following journal acknowledgement, with
the saved-project Undo preference off. It also exposed a separate working-view loss: sequence
1042 reopened as sequence 11. The recovery repair now includes validated `ProjectView` in
journals and auto-saves, tracks view changes independently of edit revisions, and restores it
through the existing preference and input validation. View-only updates retain the worker's
project/history snapshot rather than cloning the history per playhead movement. The original
focused reproduction passes; broader regressions and a repeated process test requiring exact
view restoration are in progress. Evidence lives under ignored
`target/process-recovery-20261031/` and `target/recovery-view-20261031/`. This does not qualify
the native recovery dialog or physical power-loss behavior.

Precommit review found that sending one view payload per playback frame over the existing
unbounded channel could accumulate memory while the writer stalls. The actual persistence
transport regression retained 10,000 notifications before the fix. A per-snapshot latest-view
mailbox now retains at most one pending notification; its lock is released before comparison,
serialization or file I/O. Three focused tests pass: the stalled receiver retains one notification,
the newest value is delivered and subsequent updates rearm it, and an old snapshot cannot update
a new one (clean projects ignore view-only updates). All 22 autosave and six sequence-tab tests
and strict engine lint also pass after the fix. The rebuilt two-process probe restores exact
serialized project and view state, active sequence 1042, and Undo/Redo; original and saved-copy
hashes remain unchanged. The earlier 4,176-test run predates the mailbox fix, so final repository
qualification is required against the bounded revision. Native dialog and physical power-loss
behavior remain separate, unqualified scopes.

Current development evidence is retained under ignored `target/console-overload-fix-20261030/`
and `target/crash-review-20261030/`. Native acceptance on a disposable copy of the attended
533-clip recovery passed: nine Console MCP pages returned every clip identity once (largest
compact page 16,854 bytes), legacy inspection routing stayed compact, stale pages were rejected,
and Undo restored the test timeline. Real Codex reported 533 clips; terminating only the owned
Codex child left the editor responsive, recorded diagnostics, and allowed reconnect. The original
recovery hash was unchanged. Evidence: `native/20261030-151646/result.json` beneath the repair folder.

The first complete test run passed 4,045 tests and failed two audio timing tests during a concurrent
feature build. Both passed individually from the same test binary. The repeated combined
`cargo xtask ci` gate, without a competing build, passed all 4,047 tests (49 skipped), formatting,
strict Clippy, layering, asset checks and WebAssembly checks (49 crates). The app and CLI were
built together with Whisper and neural voices enabled; native acceptance used those binaries.
These checks qualify this local macOS repair, not every platform or format. The original later
editor exit remains unexplained; the controlled child-failure test is separate evidence.

### Voice expiry and provider-retry recovery (2026-10-30)

The voice controller could omit a remote Stop after an active session cleared its startup timer,
and a later failure could clear the cleanup deadline while leaving a pending Stop. Repairs send
Stop for active sessions, retain a bounded cleanup deadline, and wait for the voice-closed
notification before reusing the conversation. If closure cannot be confirmed, an explicit error
requires reconnecting Codex; the text connection is not terminated. Provider error details are
bounded and redacted in diagnostics.

Retained app logs also showed `responseStreamDisconnected` / `Reconnecting... 2/5` being treated as
fatal. The installed App Server schema includes `willRetry`; retryable errors now leave the live
turn and pending native result intact. Exhausted retries fail the turn without killing the
connection or replaying an edit. Unit/UI tests cover both paths and stale-turn notifications.

The local generated-speech baseline passed against a real provider. The opt-in lifecycle test
also passed: actual worker expiry with a 20-second test-only lifetime, confirmed remote closure,
local audio-slot release, voice restart on the same conversation, and a subsequent text response.
Production microphone lifetime remains one hour. The synthetic MCP fixture refuses every tool
call, so these tests do not qualify project editing, physical devices, render-load audio quality,
or one-hour endurance. See the [voice test instructions](../apps/filmcraft/src/codex_console/voice/README.md).
Private evidence is in `target/filmcraft-notes-goal-20261030/`, including
`voice-live-worker-expiry-reproducible.log`. The final combined `cargo xtask ci` gate passed
4,054 tests (50 skipped), strict Clippy, formatting, layering, assets and WebAssembly checks
(49 crates). A before/after hash check confirmed unchanged source during that run. Packaged-demo
qualification passed for the signed Voice Recovery bundle on a disposable 533-clip project copy:
all clip identities across nine pages, edit/Undo restoration, real-provider inspection, owned
Codex-child failure with editor survival, diagnostics and successful reconnect. The original
recovery hash stayed unchanged. Evidence: `native/20261030-210558/result.json` beneath the repair
folder. Microphone capture remained off; generated-audio voice lifecycle qualification is separate.


### Bounded macOS drawable acquisition (2026-10-30)

Two retained thread samples place the UI thread in Metal `CAMetalLayer::nextDrawable` waiting on
a semaphore. They establish the blocked call, not whether focus, captures, Codex or voice caused
drawable starvation. The later attended background tests did not reproduce the incident.

The retained `wgpu-hal` 30.0.1 dependency enables its existing drawable timeout on macOS only.
Apple documents an approximately one-second wait followed by nil when enabled; the existing
backend maps nil to a recoverable timeout. Existing occlusion handling and other Apple-platform
policy remain intact. No project unsafe block or OS binding was added, and AGENTS.md is unchanged.
Provenance, the registry archive SHA256, original licenses and the limited diff are retained in
`vendor/wgpu-hal/FILMCRAFT-PATCH.txt` and `FILMCRAFT-PROVENANCE.json`.

The native CAMetalLayer property regression fails with the upstream false policy and passes with
the patched initial/reconfiguration policy. `cargo xtask ci` runs it explicitly on macOS because
the published third-party dependency is outside the project workspace. The test opens no window
and uses no user project. It verifies the actual QuartzCore setting, not a reproduced starvation
or a guarantee against driver/kernel failure.

The signed Graphics Recovery package passed native hide/restore, background control and
window zoom/unzoom on a disposable 533-clip project. Hidden control responded in 0.389 s and
restored control in 0.035 s; window geometry changed from 1600×963 to 1920×969 and back.
Nine MCP pages returned every clip identity exactly once; stale revisions were rejected and
Undo restored the original test timeline. The original recovery file hash was unchanged.
Evidence: `target/filmcraft-notes-goal-20261030/native-graphics/20261030-214618/result.json`.
Codex and microphone were off. This qualifies those window lifecycle checks, not reproduction
of drawable starvation or general graphics/driver reliability.


### Console cold-start conversation continuity (in validation, 2026-10-30)

Connect resolves the current project archive before choosing a thread. It resumes an active
conversation or the latest archived conversation, including one last used with another project.
Current project memory replaces old memory, and the startup instruction requires fresh inspection,
forbids replaying old edits and invalidates previous approvals. Choosing a default does not mark it active before cached messages are restored.
New Conversation remains an explicit fresh-thread request. A failed resume reports retry/new
conversation choices and does not fall back to thread/start or replay previous edits.

Regressions cover unsaved-project restart/first-save continuity and cached messages, a shared
archive whose previous conversation belonged to another project, explicit New Conversation, and an
App Server rejection that emits only the original resume request. Full qualification is pending.

### Console inline captures and microphone mute (in development, 2026-10-30)

Contract 6 adds actual image content delivery for the visible Timeline, including bounded
track/clip identities, visible/captured tick ranges, zoom and document identity. Focused ranges
must already be visible; capture never pans/zooms implicitly. Inline capture rejects stale views
and mismatched document epochs/revisions. AI captures use no screenshot files; explicit screenshot
exports are separate. PNGs shrink to a bounded delivery size and report actual/original dimensions.
Provider history is outside FilmCraft's screenshot-file retention policy.

Microphone mute bypasses the busy command queue, clears input backlog across transitions and
keeps the speaker output independent. Copy preserves Console output; Paste targets the draft
without sending. The source-stable authorized full gate passed 4,074 tests (50 skipped), doctests, strict Clippy,
layering, assets, the native Metal property regression and 49 wasm crates at `d57b07ed`.
Evidence: `target/console8-gate-retry-20261030/ci-result.json`. The separate full-feature Console
Recovery package is signed, deep-signature-verified and both binaries pass `--version`.
Packaged native qualification remains pending, including known-waveform interpretation and
clipboard/microphone behavior. No live
user microphone was opened for the source tests; no release-stage improvement is claimed yet.

### Command and connection diagnostics (in validation, 2026-10-30)

Three synthetic UI tests verify payload privacy, history/batch bounds, pending outcomes, retained
last revision change and underlying-command identity through the actual control dispatcher.
A source-stable combined run passed all 37 selected diagnostics/connection/voice regressions,
including fixed setup metadata and previous voice cleanup/provider retry repairs. Evidence:
`target/filmcraft-notes-goal-20261030/diagnostics-combined-result.json`. Logging the same fixed
command metadata was added afterward; the full gate and native UI qualification remain pending.
No microphone was opened, no live prompt sent and no user project changed by these tests.

### Bounded font-name previews (in validation, 2026-10-30)

Temporary SFNT/TTC extraction and direct shaping/rasterization avoid registering preview faces
or filling permanent font/layout/glyph caches. Focused tests cover real bundled faces, corrupt
sizes/truncation/bit flips, missing/restored files and regular-file requirements. Optional input
at craft-fonts commit `aced4137c022f1f587624d48e81ec77e1914a9cc` was fetched separately;
all 13 manifest hashes and OFL notices were verified. Eight script-font cases rendered with
that input; the focused optional run passed ten tests. No new font files enter this repository.

The shared picker uses one guarded native worker, bounded request/result queues (eight each),
32 cached textures and four result/request operations per frame. Completion timestamps prevent
hidden-app delivery delay from being mistaken for slow rendering. Stale IDs, timeouts, invalid
image buffers and device limits leave a selectable text label. Wasm renders one bounded preview
per frame without filesystem discovery. Seven worker/widget tests passed, including actual
worker pixels and an accessible selectable row; twelve catalog tests passed after adding
bounds and preserving first-face identity. These checks are not native-window acceptance.

Full app gates passed with `CRAFT_FONTS_DIR` unset and with the verified optional input enabled:
4,098 tests passed and 50 skipped in each configuration, plus strict Clippy, doctests, layers,
assets, native Metal policy regression and 49 WASM crates. The source hashes remained stable;
local evidence is `target/font-preview-matrix-20261030/result.json`, source commit `83d70e07`.
Existing caption-picker integration is part of that gate. Native visual/interaction and packaged
qualification remain pending; the currently attended demo is not replaced by these source changes.

Packaged Console image acceptance subsequently exposed a panel-ID mismatch: actual panel routing
uses `panel.Timeline`, while capture recognition compared a lowercase ID. The generated waveform
was visible and MCP returned PNG content, but timeline metadata was absent. This is a failed
acceptance result, not proof of working focused capture. The repair derives recognition from
`PanelKind::Timeline.id()` and tests the real control route and image reply; native and provider
acceptance must be repeated before claiming completion. No screenshot files were retained.

### Signed Timeline Recovery acceptance (2026-10-30)

Source `52020ab4` passed the source-stable full gate (4,100 tests passed, 50 skipped; strict Clippy,
doctests, layers/assets, native Metal policy and 49 WASM crates). The full-feature app and CLI were
packaged separately, signed and hash/version checked; optional fonts carry their original notices.

On a generated ten-second waveform fixture, actual Console MCP returned full and focused PNG
ImageContent with revision/epoch, waveform visibility and range metadata. Offscreen ranges failed;
project, playhead, selection and timeline view remained unchanged. The actual Codex connection then
read activity at approximately 1–2, 4–5 and 6–8 seconds, matching the private fixture definition;
these intervals were absent from the prompt. It explicitly distinguished visual reading from listening.
No screenshot files were retained. Native output Copy/Paste filled only the draft, diagnostics showed
fixed command metadata, and Disconnect/Connect retained the same isolated conversation. In Graphics
Properties, distinct font previews scrolled correctly; choosing Georgia changed rendered text and
Undo restored Inter. Other font selectors were not separately native-tested.

Local artifacts: `target/filmcraft-notes-goal-20261030/native-console-images/20261031-023722/`.
The original generated fixture remained unchanged; no user project was opened. Physical microphone
mute/reply behavior, cold-start default resume and failed-resume acceptance remain separate checks.

### Export finalization progress (in validation, 2026-10-30)

Progress now distinguishes processing from finalizing; phase is work stage, not an outcome. Real
media-unit counters can reach their total before flush/mux/write completion. Job and queue JSON expose
the phase, the UI says Finalizing, and ETA remains unknown during this unmeasured tail. Cancellation
remains available, and batch producers restore processing for the next segment. Focused regressions
passed for a synthetic thirty-second tail, a real stepped export's last-frame boundary/cancellation,
and status-bar/queue presentation. A separate Job JSON regression covers result publication.

A private five-second 320×180 H.264 fixture, without audio and with two software worker threads,
was traced in single/two-pass modes with FilmCraft's per-crate development optimization settings.
Local runs took approximately 0.118/0.169 seconds; the last-unit-to-file-completion tail was about
1 ms in both. All numeric ETA readings were unknown because these jobs finished before the
one-second estimation warmup. This validates phase/count tracing, not full-song numerical accuracy,
and does not explain the reported large estimate error. No estimator multiplier/window tuning is
justified by this trace. Evidence: `target/filmcraft-notes-goal-20261030/export-trace/summary.json`.
Full gates and full-song forecasting qualification remain pending.


## Live unrendered playback and GPU Noise (2026-10-30)

The reported problem is live video of unrendered clips, not export. An isolated copy of the user's
latest autosave reproduced the slowdown at 80 seconds of the vertical maximalist sequence, with
Program maximized and playback at half resolution. Ten-second native measurements showed 79 frames
shown / 222 dropped with the adjustment enabled, versus 249 / 57 with only that track disabled.
Both had zero audio underruns and roughly 60 fps UI; neither created preview renders. Removing the
adjustment is diagnostic only. A smaller Program view at a different section was smooth, so that
result cannot qualify the heavier workload. Original projects and recovery files were unchanged.

`gpu_noise_preserves_cpu_field_at_exact_timestamps` compares actual Metal output against the
production `effects::apply` path on transparent/partial-alpha/opaque pixels, colored/monochrome,
clipping on/off, a fractional grain size at reduced scale, and seeds across the u32 boundary and
negative Tick representations. The shader emulates the existing wrapping u64 hash with u32 limbs;
it does not substitute a different random field. The full 13-test GPU effects suite passed on Metal.
`colored_noise_negative_timestamp_does_not_overflow_seed` failed on the original colored-seed
addition and passes with wrapping offsets; the existing noise cadence/determinism regression passes.
A sandboxed run without an adapter is not counted as hardware evidence.

GPU Noise alone does not accelerate the whole adjustment layer: the current frame planner still
falls back to CPU for adjustments and transformed nests. Ordered GPU composite stages, image parity
and matched packaged playback measurements remain required before claiming the slowdown fixed.


## Console Return submission (2026-10-30)

The focused message draft treats plain Return as the existing Send action and Shift+Return as a
newline. It consumes plain Return only inside that draft; it does not introduce an application-wide
send shortcut. IME preedit/commit events and held-key repeats cannot submit. A small visible hint
explains the keys. Disconnected, busy, voice-active and blank-draft states retain the existing guards;
provider failures keep the draft and report the existing error.

Focused tests use the actual TextEdit and Console Send/provider-hook path: Return vs Shift+Return,
held-key state, fresh-key IME preedit/commit, focus ownership, disconnected/busy/voice/empty guards,
one provider request while ready, failed-send draft retention, and unchanged project data. The
headless harness explicitly handles egui texture deltas. Full repository and native packaged
keyboard acceptance are still required; these tests do not qualify live playback.


## Native audio output calls outside the window thread (2026-10-30)

A signed release-comparison app failed its 90-second startup deadline. A one-second stack sample at
54 seconds captured every sampled main-thread tick in `CpalOut::new -> output_device ->
Device::default_config -> AudioComponentInstanceNew -> AudioDeviceCreateIOProcID -> mach_msg`.
This establishes a CoreAudio device-init wait, not the cause of the historical Metal incident or
unrendered-video slowdown. The owned app was terminated, its GUI lease released, and user projects
and recovery files were untouched. No global audio service was restarted.

The native output now lives on one worker, including construction, configuration, open/stop,
recovery and enumeration. CPAL streams stay on that thread; the backend need not implement Send.
Admission has four queued requests and one coalesced latest configuration. UI access uses cached
metadata and nonblocking admission; a blocked worker is not replaced with accumulating new threads
or joined while closing. The status bar and `ui.inspect.audioOutputNote` expose cached failures.
The existing transport clock-stall/wall-clock fallback remains available.

A playback generation gates callbacks both before and after mixing. Device format validation
precedes audible output, silent priming buffers do not consume program audio or advance its clock,
and a cancelled/late request cannot play its old mixer. The UI reads the active callback's frame
counter directly, so a blocked hardware query does not freeze a healthy audio clock. A changed
sample rate is reported before retrying the mixer at the new rate.

Nine focused synthetic tests passed: blocked construction with responsive calls/Drop, cancelled
late Start, normal clock/Stop, changed-rate silent rejection/retry, latest-settings coalescing while
Configure blocks, invalid-name error/recovery, driver panic containment, Stop racing a fill, and
blocked Stop/Recover/Devices calls. The last test uses a separate simulated window thread and a
bounded acknowledgement before releasing the fake driver, and verifies live callback-clock progress
while the worker is blocked. The fake backend is deliberately not Send. Targeted strict Clippy
passed. Full source-stable gates and actual packaged CoreAudio/startup and playback acceptance are
still required; synthetic tests do not establish audio-device recovery on this Mac.


## Selected PNG references during voice (in validation)

Contract 7 adds stable monotonic snapshot identities and epoch-checked pixel retrieval from
user-selected in-memory references. PNG admission prepares a bounded, alpha-preserving delivery
image. The image read command takes no path, does not reopen the selected source and makes no edit.
Tests cover index removal without identity reassignment, cleared references without identity reuse,
counter exhaustion without replacing references, active-voice retrieval after source deletion,
stale epochs and path-argument rejection. A separate actual MCP stream test verifies ImageContent,
decoded transparent/semitransparent pixels, snapshot metadata and snapshot-only bridge parameters.
The 16 focused reference tests and the actual MCP image transport test passed. The initial small-image
case caught unintended thumbnail upscaling, repaired by downscaling only. Hostile schema and
separate text/image-budget checks are included in the full gate. Full source-stable gates and
packaged/provider visual acceptance remain required; no physical-voice result is claimed.


## Larger PDF references and supervised parsing (2026-10-31; in validation)

Native PDF admission now uses one background worker, four queued requests, eight bounded replies
and at most four selected helper processes. UI submission/poll/cancellation never waits for file
or parser work. Snapshot identities/epochs reject stale publication; one-shot attachments clear
on Send, while loaded PDFs remain selected until Remove or project switch for page retrieval.

The helper has no project or path interface: it receives at most 32 MiB of selected bytes, parses
one snapshot, and returns at most 4096 text bytes per page chunk. Page extraction is capped at
64 KiB; eager stream decompression at 2 MiB. A conservative load filter bounds admitted object
storage estimates to 128 MiB, 65,536 root objects, one million nodes, a 65,536-node pending walk
and depth 128. These accounting limits are not an OS RSS or parser-scratch cap. A macOS child-only
DATA/AS-limit probe failed; no such limit is claimed. The parent supervises request deadlines and
bounded pipe messages; a 20-second child-side active-work guard also bounds orphaned parsing,
without expiring idle snapshots. Unconfirmed child termination retains its process-budget slot.

Fourteen exact-source helper/supervisor/service tests passed, covering malformed/truncated inputs,
mutation cases, UTF-8 offsets, invalid-page recovery, structure/storage limits, blocked drivers,
UI-side admission/poll/Drop responsiveness, cancellation, late replies, worker/process budgets,
backend panic containment, oversized replies and active-vs-idle child deadlines. Eighteen Console
reference tests passed, including ready-after-load, retained PDFs after Send, consumed page tickets,
visible failure/draft preservation, removal and stale project-switch replies without project/Undo
mutation. Four actual MCP/schema tests passed: pending-to-complete text, resuming a ticket without
duplicate reads, forbidden paths/invalid numbers and mode rejection before bridge I/O.

Exact-source child-process probes indexed the supplied Kodak PDF (216 pages), Introduction to Film
(119 pages), and an original 22,301,371-byte illustrated fixture (448 pages), returning bounded text
from pages 1/10/20 and retaining source hashes. The Kodak file exposed the old 8 KiB eager-stream
limit; an otherwise identical larger-limit candidate succeeded. An initial load-filter move broke
valid PDFs and was repaired by preserving the parser input object. The test logs retain those
failed attempts. One book run used a stale binary while rebuilding and is explicitly excluded
from qualification; the later source-stable run passed all three documents.

These are component and headless Console results. Full source-stable repository checks, actual
app helper entry-point verification, signed-package Add Files/drop during connected Codex, and
packaged/provider/native acceptance remain required before claiming this workflow complete.

### Native PDF follow-up and idle bridge failure (2026-10-31; in validation)

The final PDF source passed the full source-stable gate: 4143 tests passed, 50 skipped, with
documentation, native Metal, layering/assets and 49 browser-target crate checks. The separate signed
full-feature package's actual helper entry point loaded the original 216/119-page references and
the generated 22.3 MB/448-page book. Each retained readable first/final pages after invalid-page
errors; oversized/truncated/path-bearing/long-header inputs returned bounded errors. Source and
package hashes were retained, and existing demo packages/project were unchanged.

An owned native run selected the 22.3 MB book through Add Files while Codex was connected. The
ready label appeared, and actual packaged MCP first/final-page reads succeeded after deleting only
the disposable selected source copy. However, Codex's subsequent page 317 request failed in 1 ms
with an unknown bridge outcome and no ticket. The model did not repeat the request or invent text;
the app remained responsive and Disconnect worked. Native drop and provider acceptance are open.

The native server closes idle sockets after 120 seconds; the bridge previously kept inspection
sockets indefinitely and reused them for ticket admission. A synthetic server closing an inspection
connection reproduced the same unknown-outcome failure before the repair. Non-replayable requests
now start on fresh connections; only retry-safe inspections pool/reconnect. No uncertain mutation
is replayed. Focused tests, the full final-source gate and repeated signed native/provider testing
remain required for this follow-up repair.

The follow-up repair passed the source-stable full gate (4144 tests, 50 skipped) and a separate
signed full-feature build. One native attempt timed out while the Mac locked and app binding
stalled; it is excluded, and only the verified delayed test-app relaunch was terminated. The retry
selected the 448-page book while Codex was connected, waited 130.0006 seconds after the provider
became idle, then retrieved exact page-specific sentences from pages 317 and 448. Actual provider
tool calls completed in 34/104 ms. PDFs remained available after Send; native Remove then caused
the packaged MCP to reject the old identity with a normal selected-reference error. Project
revision/epoch and originals remained unchanged. Native drag/drop remains unqualified.

Separate signed native runs also qualified selected PNG Add Files/MCP delivery after deleting
only its disposable source copy, typed-provider colors/shapes/positions, Shift+Return newline,
Return sending once, and disconnected Return preserving the draft. Encoded alpha preservation
has transport coverage; the model described its visible black backdrop, not the alpha channel.
Voice was off. A signed Audio Recovery startup answered control in 3.4 seconds, with 30 response
probes within 13 ms, and native menu/Console interactions worked. One successful startup does not
prove repeated driver-failure recovery, physical listening or voice endurance.

## Ordered GPU adjustment composites (2026-10-31; in validation)

Matched native optimized source `70071099` on the private sequence 1042 at 80 seconds, maximized
Program, half resolution and no previews measured 90 shown/191 dropped frames with the grade
enabled (67.97% drops), versus 264/35 (11.71%) with only the adjustment disabled. Window-weighted
CPU render means were 187.6/63.6 ms; both had zero audio underruns and about 60 fps UI. The owner
reported the latest comparison looked very smooth. That attended feedback applies to the diagnostic;
the enabled-grade result is still inadequate, and harness completion is not playback qualification.

The candidate represents supported full-frame Normal adjustments as ordered `PlanStep::Adjustment`
operations. Each copies the lower-track accumulator, runs the existing GPU effect math, and replaces
all channels with the opacity-weighted result, including pixels cleared by alpha-changing effects.
Layers between adjustments remain ordered. Masks, transformed adjustment coverage, other blend
modes and unsupported effects preserve the whole-frame CPU reference; nested adjustments are not
flattened into a parent canvas. Effects-off skips adjustments. Chains cap at 256 operations and
share the existing 4096-unit plan budget with source layers; adjustment-only plans are bounded too.

GPU admission accounts for three RGBA32 working textures plus an RGBA16 original, using checked
arithmetic and a 512 MiB per-adjustment additional-storage limit. This is not a total GPU RSS cap.
Unavailable stages, excessive canvases or unsupported operations fall back to the complete CPU
picture. Effect-pool cleanup happens at whole-frame boundaries so final resolution does not discard
working textures; an effects-free frame releases unused sizes. `perf.stats.gpu.adjustments` counts
submitted GPU adjustments, excluding CPU fallbacks. Composite export keeps its existing CPU path.

Initial focused tests passed, followed by 43 GPU and 206 render library tests (three render tests
ignored). The GPU test executed on the device and compared ordered Noise/invert/crop, partial alpha,
transparent replacement, three adjustment opacities, prepared inputs, forced CPU fallback and working
texture retention/release. Sequence-renderer comparisons cover ordering, effects-off, two scales,
three opacities, masked/moved/blended/unsupported and oversized-chain fallbacks. The checked-storage
budget test was added afterward. The final combined run passed 137 export, 44 GPU and 206 render
tests (three render tests ignored), including the budget case and an actual-device GPU execution
marker. The existing export GPU-hook scenario failed before separating playback eligibility from
the export CPU shortcut, then passed after that repair: adjustment exports remain byte-identical to
the CPU path and do not first build a declined playback plan. Optional ffprobe-dependent HDR oracle
checks reported unavailable in this component run; they are not claimed as qualified by it. Final
strict/full gates and a new signed graded playback measurement remain required; transformed-nest
optimization is still under review.


## Console output selection and context Copy (2026-10-31; in validation)

The owner reported that opening the output context menu clears highlighted text. A real egui
pointer-event regression first created a visible selection, then failed on the missing highlight
when the secondary-button menu opened. The previous menu also always copied the entire message.

The repair keeps egui's existing label selection, including Unicode and selections across message
labels. Secondary-button/menu gestures do not start or extend a selection. Menu Copy requests the
selected text before the transcript's selection pass; no selection falls back to the existing whole
message command. `console.chat.copy` retains `{index}` behavior and accepts optional boolean
`selection` for the context-menu path. Paste still targets only the draft. Regression coverage
checks menu-open/menu-press highlight, exact partial/Unicode/multi-message clipboard content,
whole-message fallback, invalid selection parameters and unchanged draft/project/no Send.
All 63 Console tests passed, including all four pointer-driven clipboard cases. The original
highlight regression failed before the repair. The popup tests settle its sizing before clicking;
otherwise even the pre-existing whole-message fallback receives no button click. Run the full
source-stable gate before committing. Packaged native acceptance remains required.


## Larger persistent Console memory (contract 9; in validation)

The owner reported that the 16 KiB saved-memory limit is too small. Saved memory now admits up to
1 MiB while session context remains bounded independently. Complete small notes are preserved;
larger notes provide explicit head/tail excerpts with full text available through UTF-8 pages and
literal search. Small guarded patches avoid sending a whole large document back as an update.
Read/search continuations and patches carry scope, document epoch and memory generation. Reload
and successful saves invalidate older snapshots. Encoded tool replies are limited to 32 KiB,
including escaping and metadata, with explicit continuation rather than silent truncation.

The initial app/MCP all-targets build check, ten standalone text-operation tests, all 81 Console
checks (including exact 1 MiB save/reopen/reassembly), both MCP schema/transport tests and strict
all-target Clippy passed. The initial MCP socket attempt was denied by the sandbox; the same test
passed with local-network permission and no source change. Both cache allocations are prepared
fallibly before committing the file. Encoded replies also reserve MCP envelope space after a second
JSON text-escaping pass (44 KiB maximum, within the 48 KiB tool budget).
Session tests exercise >16 KiB save/reopen/reassembly, bounded context, search/patch, stale snapshots,
reload, escaped output, failed-write preservation, generation exhaustion, hostile parameters and
bounded editor controls. Regular-file checks and Unix nonblocking open reject replacement FIFOs
without waiting for a writer; the synthetic FIFO test has bounded failure cleanup. No unsafe or
non-media FFI wrapper is added. The final reviewed source requires the full source-stable gate
and signed native acceptance before the feature is qualified. No owner memory file, preference, project or running demo is migrated by this change.


## Signed native acceptance follow-up (2026-10-31)

The following local evidence supplements the earlier source-only or component results; it does
not replace the stated limitations. Tests used independent private fixtures and data directories
under the cooperative GUI lease, during the owner's authorized native test window. No screenshot
files were retained. Evidence paths below are ignored local artifacts, not release assets.

- **Console selection (`7d6c2807`):** the full source-stable gate passed before the signed package.
  Native pointer selection of `voice started`, secondary-click menu opening, Copy and native paste
  into the draft preserved the highlight and copied precisely that substring. No Send occurred;
  the complete transcript and project identity stayed unchanged. The owned provider disconnected
  before cleanup. `target/console-copy-selection-20261031/native-20261031-123759/result.json`.
  Unicode and multi-message cases retain their pointer-driven automated coverage; this native
  example does not claim to exercise all of those variants. Accessibility activation alone did not
  drive the controls reliably; the verified path used visible coordinate input through CUA.
- **Recovery (`233779e1`):** all 4,179 workspace tests (50 skipped), doctests, strict lint, layering,
  asset checks, native Metal policy and 49 WASM crates passed on stable source. The signed app
  journaled a private complex project with one Undo and one Redo, then a view-only playhead change
  without a revision increment. After journal acknowledgement, the parent killed only that owned
  app with SIGKILL. A fresh process displayed Recover Unsaved Changes; native Recover restored
  the entire saved project and view byte-for-byte, including sequence 1042 and its 81-second
  playhead. Undo/Redo then matched the complete expected project values with exact decimal-number
  parsing. `target/recovery-native-20261031/run-1793450667087936000/result.json`. The first run
  incorrectly compared edit-history results against older views too; retained artifacts showed
  matching projects and only the expected playhead difference. Its failed oracle is not erased.
  Physical power-loss durability is not established by SIGKILL.
- **Larger memory (`8ac1569d`, exercised in signed `233779e1`):** the source-stable full gate passed
  4,173 tests (50 skipped), plus 49 WASM crates; signed MCP schema/page/search/patch transport
  checks passed. The native editor loaded 33,075 UTF-8 bytes, appended `Native saved tail Ω` and
  a newline, saved exactly 33,096 bytes, and displayed that saved tail after process reopen.
  The complete saved notes matched expected bytes, with generated Unicode notes intact.
  `target/memory-native-20261031/run-1793450722368480000/result.json` and
  `native-observations.json`. An ineffective initial accessibility focus sent shortcuts to the
  disposable fixture; coordinate focus succeeded. Original and private saved project files were
  unchanged, and its private recovery journal was retained with Not Now. This is not an unchanged
  in-memory fixture assertion. No Codex or microphone connection was needed.
- **Graded playback (`30e0b029`):** the source-stable full gate passed before packaging. Matched
  signed native ten-second tests used the same project hash, sequence 1042, 80-second start and
  half-resolution view. Baseline showed 90 frames and dropped 191 (67.97%); candidate showed 283
  and dropped 12 (4.07%), with 287 actual GPU adjustment submissions and no audio underruns.
  Weighted frame-job render time fell from 187.60 ms to 43.23 ms. See
  `target/ordered-adjustment-playback-20261031/20261031-124030-graded/matched-comparison.json`.
  A 107.5-second observation from the start reached final frame 3202 and stopped naturally:
  2,988 frames shown, 214 dropped (6.68%), 3,026 GPU adjustments, zero audio underruns and zero
  failed frame jobs, without rendered previews. Its harness later timed out waiting for the final
  review marker; the original failed result is retained alongside `measurement.json` and
  `measurement-review.json` in `20261031-124735-full-song-graded/`. Completed measurement is
  not a passing final-review wrapper. Residual delivery drops, transformed-nest cost and the
  observed colored-shadow artifacts remain unresolved; responsiveness is not visual equivalence.
- **Full-song export and ETA:** signed release CLI `8ac1569d` has unchanged export/render/media/
  CLI sources through `233779e1`. It exported all 106.7667 seconds at 1080×1920, 30 fps, H.264/AAC,
  with GPU rendering and hardware encoding explicitly off, in 844.780 seconds (109,904,496 bytes).
  External Apple Silicon ffprobe decoded all 3,203 video frames and 5,005 audio frames with empty
  error output and matching durations/dimensions. The downloaded test-only executable matched its
  publisher's SHA-256; it is neither linked, bundled nor shipped. Original/private source files and
  complete before/after project/view snapshots matched. The initial wrapper failed only because
  ffprobe was absent after export; independent verification used the existing movie, without a
  rerender, and preserves that failed result. Evidence:
  `target/full-song-export-qualification-20261031/packaged-run-1793446324109076000/independent-verification.json`.
  The 2,586 finite ETA samples from 10–90% completion had median absolute relative error 6.7635%,
  p90 31.5820%, and median signed error −10.51 seconds. These are remaining-time errors, not total
  duration errors. Scene cost varies; one trace does not establish universal prediction accuracy.

Actual microphone-only mute with continued AI speech, real-device one-hour expiry/retry, native
PNG/PDF drop and voice/render continuity remain pending. API/Desktop connections are isolated and
unmerged; fake-backed transport checks do not qualify live provider, saved API resume or voice.


## Nested working-image placement candidate (2026-10-31; in validation)

A matched cold-start pair at 55 seconds, with the same signed binary and ten-second half-resolution
observation, showed 144 frames/154 drops with masks versus 162 frames/137 drops without opacity
masks. Both had zero audio underruns. Only the private diagnostic copy's overlapping opacity masks
were removed; grading, nesting and motion remained. This changes the picture and is not a fix.
Evidence: `target/masked-playback-diagnostic-20261031/paired-comparison.json`. Cold starts are not
directly comparable to the warmed full-song intervals.

The candidate separates a nested clip's CPU working image (nested composition, effects and masks)
from its final placement. The same helper supplies the CPU reference; the GPU receives the working
pixels and matrix only for axis-aligned source bounds covering the output with a one-pixel margin.
CPU placement remains for visible borders and rotation. The initial unrestricted actual-device
comparison failed the unchanged p99 tolerance (20 codes for a small rotated/minified source), even
though CPU plan/reference matched exactly: GPU quad clipping differs from the CPU transparent
bilinear border at geometry edges. The failed evidence is retained in
`target/nested-placement-20261031/gpu-placement.log`; no tolerance was relaxed. The guarded
actual-device matrix passed all twelve scale/mask/transform cases with p99 at most one code.
The regression failed on the previous identity-matrix CPU-canvas plan before the repair; all nine
nested CPU tests passed afterward. The later aggregate-frame-storage guard requires component
and full-gate requalification. Retained frame plans have a 512 MiB frame-byte budget and reject
oversized new working images before rendering them; rejected optimizations use the CPU path.
The final frame-plan fallback releases retained layers before rendering the whole CPU image.
No native performance improvement is claimed before a new signed matched measurement.


Upload-cache review found two related issues before packaging: the previous 24-entry-only cache
could retain large working images without a byte budget, and eviction during upload could remove
an earlier layer in the same batch before its bind group was created. A tiny actual-device
25-layer regression reproduced the latter: an opaque red first layer followed by 24 transparent
layers rendered black instead of red. Evidence: `target/nested-placement-20261031/cache-before-fix.log`.
Eviction now occurs after every current batch layer has its own bind group/texture references,
retaining the complete picture while bounding the reusable cache to 24 entries and 256 MiB.
Accounting includes retained CPU pixel bytes and uploaded texture payload bytes. An entry larger
than the cache budget is usable by its current draw but is not kept cached. This cache budget is
not a claim that the entire renderer, frame plan, in-flight GPU commands and decoder caches together
fit within 256 MiB. The separate frame-plan budget remains 512 MiB. Dedicated tests cover exact
small-frame accounting, LRU eviction, oversized-entry eviction and the 25-layer picture. The earlier
owned full-gate group was verified and deliberately stopped (exit 143) before applying this repair;
its source checkpoint and log remain in `pre-cache-review/`. It is not a passing final gate.

All 47 actual-device GPU tests passed after the cache repair, including the nested placement and
25-layer regressions. The final source-stable full gate passed (4,185 tests, 50 skipped;
49 WASM crates). Signed native performance remains required.


## Black-anchored shadow continuity candidate (2026-10-31; in validation)

A generated near-black RGB ramp with small channel differences reproduced a sharp blue/channel
jump in the existing CPU Lumetri path with negative Blacks and positive Shadows. The old-code
regression fails at pixel 453: blue jumps from roughly 0.000019 to 0.191 in linear working output.
After black-point subtraction, channel values can straddle zero while luma approaches zero. The
old positive shadow lift stays nonzero, and dividing by luma above a hard 1e-5 cutoff abruptly
amplifies the channel residue. CPU and GPU shared this formula, so parity alone cannot qualify it.
Evidence: `target/shadow-tone-fix-20261031/before-fix.log`.

The candidate smoothly anchors the shadow weight with a quadratic smoothstep toe below 5% of the
encoded grading signal, removes the division-floor discontinuity, and retains the original weight
above the toe. Negative/zero luma is not divided. Reference CPU, planned effects and WGSL use the
same formula. This deliberately changes the faulty near-black response for existing shadow grades;
it is an editor curve, not an emulation of a film stock. HDR uses its normalized grading signal,
not a claim that 5% is a particular luminance or reflectance.

An isolated prototype covered 80 finite negative/SDR/overrange input cases. The largest adjacent
channel jump fell from 0.667 to 0.000592 and neither version produced non-finite pixels in that
matrix. These are synthetic diagnostics, not the owner's exact offending frame and not HDR/GPU
qualification. Production tests separately require a continuous near-black ramp in the reference,
planned and actual GPU paths, CPU/GPU agreement, exact weight preservation above the toe, and
finite/black-anchored PQ/HLG shadow output. All ten focused CPU/HDR tests and the actual-device
GPU regression passed, including both cool and neutral white-balance cases with an absolute
CPU/GPU output difference below 1e-4. No GPU-adapter skip occurred. Full gates and packaged visual
review remain pending.
The owner's project, grading settings and source media have not been modified by this investigation.


The initial complete workspace run exposed one expected preset-grid mismatch: fifteen dark neutral
ramp pixels in Hard Sun and Lift Shadows changed with the new toe. Expected/actual/diff images and
pixel coordinates were reviewed; all other pixels, dimensions, layout and alpha matched. The old
reference and failing gate remain under `target/shadow-tone-fix-20261031/pre-golden-refresh/` and
`old-lumetri-presets-grid.png`. Only the original procedural preset-grid reference was regenerated
by its compiled renderer test, with its existing licence/index and updated attribution notes. The
existing tolerance is unchanged. Its ordinary comparison then passed; the full gate must rerun.


## Actual one-hour voice lifecycle (2026-10-31; generated audio only)

The real signed-in provider test ran for 3,607.5 seconds with a 3,600-second first worker lifetime.
The unchanged copied production controller and transport received the one-hour expiry, acknowledged
remote Stop and released the worker slot. A second generated-audio voice connection succeeded on
the same conversation, stopped cleanly, and a subsequent text turn returned successfully. The
fixture's MCP tools reject every action; no owner app/project, microphone or speaker was opened.
Evidence: `target/voice-one-hour-20261031/result.json` and `runtime.log`, with binary, generated PCM
and copied-source build provenance retained beside them. This proves the elapsed-hour controller/
provider lifecycle, not physical-device health, native control availability, microphone-only mute,
continued audible AI speech or render-time voice continuity. Those native/device checks remain open.


## Bounded microphone metadata and capture candidate (2026-10-31; in validation)

Opening disconnected Command Console in the signed Recovery View test bundle caused the native
AX action and `ui.inspect` to time out. A three-second sample of the verified private PID shows
Console drawing calling `AudioInput::channels`, CPAL default input configuration and
`AudioComponentInstanceNew`, blocked in a CoreAudio mutex on the main thread. Codex and voice were
off; no file drop was attempted. Only that owned instance was closed after sampling. Evidence:
`target/input-device-hang-20261031/finding.json` and the referenced sample/result directory.

The candidate moves input names/channel discovery to one worker with a 16-key cache, 16-key
pending queue and one wake signal. Results, names and errors are bounded; stale host/device results
cannot be returned for a different key. UI reads use cached metadata, with an actionable two-second
stuck-query diagnostic. Discovery errors are separate from captured-take errors. The existing
`audio.voiceover.settings` response adds `inputDiscoveryPending` and `inputDiscoveryError`.

Capture startup waits at most two seconds. Stop/Drop cancel callbacks and signal the worker without
joining an unfinished driver operation. A process-wide input-capture slot remains held through
actual teardown, preventing retries from accumulating blocked workers. Late-open activation and
callback buffer/error writes check their cancellation gate; callback failure reporting performs no
log I/O while capture locks are held. Existing capture buffers and failed-save retry behavior remain
intact. Neither discovery nor capture claims it can interrupt a stuck operating-system call.

Thirteen copied-implementation tests passed for blocked discovery/startup/teardown, panic recovery,
late cancellation, independent default/named device queries, cache/queue/metadata bounds, retry
limits, invalid requests and stale callbacks, without opening physical devices. An old Console
regression fails because pending zero channels become one; the integrated regression requires zero
to remain pending and later update to the actual count. Voice-over dialog tests also require late
metadata refresh to preserve unsaved preferences. The integrated source then passed all 13 actual
input-module tests, two UI discovery tests and nine engine recording/save tests with unchanged
source hashes. Full gates and signed native Console open/close/reopen, physical microphone and
control checks remain required; these component tests used fake drivers and opened no devices.

The first matched playback baseline in the new session remained responsive but reported unavailable
audio hardware and zero callbacks; its video measurement cannot qualify audio health. The candidate
measurement was not obtained: a native automation call stalled for 3,466.7 seconds, outlasting the
private wrapper's wait for native Play. Its late relaunch opened a default test-bundle instance,
which was identified and closed. That attempt is retained as inconclusive, without attributing an
app playback defect. Native harnesses now use test-only private launch guards, validated against a
local argument recorder and a separately signed bundle. Exact unsigned executable equality is
checked on disposable copies, so the guard changes routing/signing metadata, not renderer code.


## Signed input recovery and residual playback measurements (2026-10-31)

Commit `77721138` passed the source-stable authorized full gate: 4,203 workspace tests, 50 skipped,
strict Clippy, doctests, translations, native Metal policy, layers/assets and 49 WASM crates. The
first sandboxed run retained 33 local-socket permission failures; all 33 passed in the authorized
rerun without source changes. Both runs remain separate evidence.

The full-feature signed release passed native disconnected Console open, close and reopen,
including expansion after reopening. Thirty control probes completed within 21 ms. Cached input
metadata reported 12 channels before and after, with no discovery error. The generated project,
revision/epoch and Voice-over preferences stayed unchanged. No capture or provider connection was
started. Evidence: `target/input-device-hang-20261031/native-1793477749474267000/result.json`.
The earlier automation-stalled attempt remains inconclusive; only verified private test processes
were closed. This proves this native metadata/reopening path, not physical recording or voice mute.

A new matched playback pair used separate signed baseline/repaired builds, the same private
sequence 1042, a 55-second start, ten seconds at normal speed, half resolution, and enabled grading
and masks. The baseline delivered 120 frames and dropped 179; the repaired build delivered 114 and
dropped 185. Both had healthy hardware audio callbacks and zero underruns. No rendered preview or
project edit was made. The roughly 60% versus 62% video-drop result does not establish improvement
in this difficult interval. Evidence: `target/input-device-hang-20261031/matched-playback-review.json`.
The owner separately reports that the demo is substantially better overall.

A diagnostic-only playback stack sample shows frame rotation, YUV conversion, masks/transforms
and Rayon synchronization. It does not turn wait counts into CPU percentages. An isolated tiled
rotation prototype passed 198 exact byte/word/float-bit cases but produced mixed timings across
six HD/4K/packed cases, including slower cases; it was not integrated. Evidence:
`target/rotation-locality-probe-20261031/result.json`. Scheduling/cache investigation remains open.

## Initial single-provider combined-source qualification (2026-10-31)

The isolated Desktop/API provider source and capture-discard correction passed a source-stable
full gate with 4,260 tests, 50 skipped and 49 WASM crates. Main now combines that frozen 34-file
patch with the input-discovery repair. The Console merge preserves capability guards, pending
zero channels, periodic cached refresh and active-voice channel preservation. Tests retain the
pending/late-channel regression with an explicit voice-capable connection and add a no-voice
provider check that must perform no device queries. The combined source requires its own full
gate and packaged checks; the isolated result is not combined acceptance.

Captures are delivered once and removed from retained/replayed RAM and journal checkpoints,
including failed/cancelled/interrupted paths and legacy checkpoint loading. Selected references,
including document epoch zero, remain separate. Native edit outcomes are never automatically
replayed after uncertainty. Actual Desktop attachment/voice and API authentication/model/image
acceptance remain unqualified; no live paid API request or key entry occurred in this integration.


The owner explicitly accepted current playback for alpha and asked to track the four-view stress
case for later. Its measured limits and diagnostic artifacts remain on the roadmap; no further
stress experiments or production rendering changes were made after that acceptance.

Combined-source focused validation passed with unchanged source during its run: 14 Console chat
cases and 43 fake-backed API provider cases, including capture-discard journals, resume ownership,
pending channel refresh and unsupported-provider device-query exclusion. Evidence:
`target/provider-integration-20261031/component-results.json`. At that checkpoint the full combined gate was the next
qualification step. Command Console separately received owner approval for Desktop qualification
and a small generated-data-only API smoke test with secure local key entry; its live result and
corrected-build provenance are still pending. No key is recorded in this worktree's evidence.


## API helper catalog regression (2026-11-01)

The attended `0978bfe1` API Connect check failed before authentication: the actual packaged helper
advertised Desktop's `connection_rebind` in addition to the 17 Console tools. The before-fix Rust
regression reproduced the 18-versus-17 mismatch. The repair validates the complete catalog, excludes
only that Desktop lifecycle tool from API advertisement and dispatch, and rejects other unknown
tools. Duplicate names, malformed schemas, oversized descriptions/catalogs, pagination and an empty
API catalog remain errors. This does not grant API access to Desktop reconnect behavior.

Run the packaged-helper regression explicitly alongside the ordinary API tests:

```sh
FILMCRAFT_TEST_NATIVE_HELPER="/absolute/path/to/matching/filmcraft-cli" \
  cargo test -p filmcraft packaged_api_catalog_excludes_desktop_rebind -- --ignored
cargo test -p filmcraft ai_provider::
```

The explicit test starts only the local MCP helper with synthetic tokens and reads its catalog;
it does not read a user key, submit an API request, contact an owner conversation or mutate a project.
It is ignored in the default suite because it requires a separately built matching helper. Source
tests do not replace the corrected package's native startup and owner-key marker/Undo acceptance.
Before-fix evidence: `target/api-catalog-repair-20261101/before.log` and `reproduction-catalog.json`
in the same directory. The corrected full gate, package and live acceptance are tracked separately;
this regression alone does not qualify them.

## Alpha acceptance follow-up (2026-10-31)

This follow-up supersedes earlier pending-build and unmerged-provider status statements above.
It does not reopen completed requirements or declare the overall goal complete.

- **Combined provider source/package:** commit `94b40395` passed the full source-stable gate:
  4,279 tests passed, 50 skipped, formatting, strict Clippy, doctests, translations, the native
  Metal policy check, layers/assets and 49 WASM crates. The full-feature release archive matched
  its source manifest before/after build; signatures and app/CLI versions passed, with the previous
  demo unchanged. Evidence: `target/provider-integration-20261031/final-gate/ci-result.json` and
  `target/provider-integration-package-20261031/release-package.json`.
- **Exact packaged checks:** CLI MCP initialization/catalog/project inspection and a private
  save-copy matched exactly. Native selector checks verified Codex default, explicit API setup
  error without credentials, external Desktop capability controls, return to Codex with actual
  inspection, and revocation/removal of the private Desktop descriptor. The project stayed
  unchanged; no microphone or live API request was used. Evidence: `headless-smoke-result.json`
  in the package directory and `target/provider-native-20261031/native-1793484353555943000/result.json`.
- **Isolated live API:** five owner-approved generated-data `gpt-6-luna` checks passed native
  inspection, restart/resume, selected-image interpretation, timeline capture without persisted
  pixels, and sanitized-history follow-up. The retained executed harness/setup verify expected
  app hashes, and the app/CLI bytes were preserved before rebuilding. This is retrospective
  retained-harness linkage, not an independent OS execution audit or combined-package acceptance.
  No key was stored. Evidence: `target/provider-integration-20261031/live-api-report-review.json`
  and `live-api-provenance-review.json`.
- **References and styles:** connected native Finder drop of generated PNG plus 22.3 MB/448-page
  PDF passed snapshot admission/indexing with unchanged project/source files. Native preset Import
  and Load matched the visible selection and effective style context without timeline edits.
  These signed-package checks close the reported connected-session workflows; dropping during
  a busy reply was not established as an additional owner completion requirement and is untested.
  Evidence: `target/attachment-drop-native-20261031/retry/native-1793483126947202000/result.json`
  and `target/style-preset-native-20261031/native-1793482994930086000/result.json`.
- **Shadow visual:** the exact signed combined app displayed generated dark neutral/subtle-color
  ramps under the negative-black/cool-grade regression settings at Full and 1/2 Program resolution,
  without an abrupt colored stripe observed. Project bytes/revision/epoch stayed unchanged and no
  screenshot files were retained. This complements the CPU/GPU regressions; the original owner's
  exact artifact frame remains unavailable/unreproduced. Evidence:
  `target/shadow-native-20261031/native-1793484665109031000/result.json`.
- **Owner alpha decisions:** current playback is accepted, with the difficult four-view stress case
  deferred. The measured export ETA (6.76% median and 31.58% p90 remaining-time error on one full-song
  workload) is also accepted for alpha; further cross-workload accuracy is deferred. Neither decision
  claims universal performance or accuracy.

Physical microphone/speaker, mic-only mute with AI speech audible, absence of the muted test phrase
after unmute, same-conversation native Stop/restart, and a voice-time `reference_image` call passed
with owner confirmations on the signed combined build. The overall native harness remains failed:
a later command-field focus error dispatched shortcuts into the disposable in-memory project.
The owned app was closed without saving; owner files and the disk fixture were unchanged.
Voice/render overlap and final native Disconnect were not qualified by that run. Evidence:
`target/physical-voice-native-20261031/native-1793485333368343000/voice-scope-review.json` and
`voice-image-tool-proof.json`. Physical one-hour endurance is not claimed.
Actual Desktop model text/edit/reconnect/voice and remaining live API edit/Undo acceptance remain
open. Desktop discovery was reproduced consuming an active lease that expires when the inventory
helper exits; its first-native-call activation repair is separate from package `94b40395` and remains
under review/qualification. Do not treat passive inventory or synthetic host tests as actual Desktop
model acceptance. Premiere import and broader portability remain on hold.


## Current alpha acceptance follow-up (2026-11-01)

Historical pending statements above describe those earlier builds; use this section for the latest
scope. The whole goal remains incomplete.

- `acc02d4a` passed the source-stable full gate: 4,281 workspace tests, 50 skipped, 49 WASM crates,
  formatting, strict Clippy, doctests and the remaining repository gates. The first sandboxed run
  failed on denied loopback binds; that result is preserved separately. The authorized rerun passed.
  Evidence: `target/desktop-discovery-main-gate-20261031/ci-result.json`.
- The exact immutable-source signed app/CLI passed signature checks, CLI inspection/save-copy and
  native selector/default-Codex/descriptor cleanup/Disconnect. The owner-driven persistent launcher
  later supplies the authenticated local control connection without a disposable runner owning
  the app. A prior test handoff stopped its observer and the app exited about 16 ms later, with no
  matching app panic/crash report found; that incident is recorded as likely runner teardown, not
  proof of an editor crash. Evidence: `target/desktop-discovery-package-20261031/release-package.json`,
  `target/desktop-recovery-native-20261031/native-1793488387955445000/result.json`, and
  `target/project-switch-exit-20261101/cause-review.json`.
- A representative owner-driven H.264 export completed 3,201 frames in 164.414 seconds. Voice was
  active for over 84 seconds while export progressed. The owner clarified that small baseline
  breakup persists, with no improvement or worsening during export. Native Voice Off was observed
  while export continued; project revision/document epoch stayed stable through completion. This
  qualifies no export-induced worsening, not perfect audio or a universal performance claim.
  Evidence: `target/persistent-ai-demo-20261101/voice-export-acceptance.json` and metadata JSONL.
- Actual Desktop conversation inspection and owner-attested external voice use reached native
  commands. Logs independently confirm marker addition and Undo; all original six marker rows
  were exactly restored. This is not a mocked-client acceptance claim. Evidence:
  `target/persistent-ai-demo-20261101/desktop-live-acceptance.json`.
- **Reconnect failed:** the same Desktop conversation repeatedly received a revoked-provider error
  after owner-reported reconnect, also after ending voice; a new chat also failed in the live
  attempt. An isolated installed-client reproduction using the exact signed CLI shows that
  `config/mcpServer/reload` succeeds but leaves an existing thread's helper stale, while a new
  ephemeral thread after reload gets the new permit. No model/API call, GUI or owner project was
  used in that reproduction. Evidence:
  `target/desktop-existing-chat-reconnect-20261101/installed-client-reload-probe/report.json`.
  The integrated generation-safe `connection_rebind` candidate passed a separate installed-client
  probe in one ephemeral thread: initial inspection, stale-permit rejection, reload still failing,
  explicit rebind, successful inspection, unchanged-permit rejection and another successful read.
  The matching CLI SHA-256 is `1fe82c6463ba6029bc03217cf63f4ff4c3409a07ed6a969f30461acf44e6fed5`.
  This proves that synthetic-host direct-tool scope, not an actual Desktop model conversation.
  Evidence: `target/desktop-existing-chat-reconnect-20261101/integration/installed-client-rebind-result.json`.
  The final isolated source passed all 89 focused tests. Exact integrated source then passed
  the full gate as `a59e1c97`: 4,297 tests passed, 50 skipped, all doctests, strict Clippy,
  translation reporting, native Metal timeout policy, layers, 69 attributed assets and 49 WASM
  crates. The before/after source hashes match. Evidence:
  `target/desktop-rebind-main-gate-20261101/ci-result.json`. Its full-feature release build uses
  a verified immutable archive, SHA-256
  `6c5c22844bb5ffa9c0205f6666b0af5c5e763dd8e524fc3a32933eda85febadb`.
  Package signing and actual Desktop reconnect acceptance remain open.
- Root UI candidates stop background `ui.inspect` from ordering a native window forward, explain
  Desktop's waiting-for-tool state, name the selected provider on Disconnect, and remove the
  unavailable embedded composer. Both dispatch regressions passed in the full `a59e1c97` gate;
  native front-to-back window order and the packaged controls still require verification.
  A signed generated Focus Witness is prepared but not launched, so the native check need not
  inspect unrelated user documents. Evidence:
  `target/desktop-rebind-native-20261101/focus-witness-package.json`.
- The speaking-voice picker is a separate integrated working-tree candidate, not part of that
  reconnect archive. A read-only installed-client catalog query returned v3 choices including Sol;
  it did not establish personal-default inheritance. Six helper tests passed against the exact
  catalog/request and response-routing functions in an isolated harness, with a 100 ms harness
  timeout. No microphone, GUI or model turn was involved. The full app/UI/preferences candidate
  remains uncompiled and unqualified. Evidence:
  `target/voice-selection-candidate-20261101/routing-harness-result.json`.

Still open: packaged same-chat Desktop reconnect and actual-client acceptance; live API marker/Undo
with secure fresh key entry; investigation/alpha disposition of small baseline voice breakup; the
new speaking-voice selector request. The owner resolved the A3 selection note. Playback stress and
further ETA accuracy remain owner-deferred; Premiere import and broader portability remain on hold.

Codex conversation lock qualification: storage tests cover independent build
state reading the same policy, explicit unlock, refusal of another/new thread,
bounded malformed data, invalid IDs, and preserving the old policy on invalid
writes. The UI integration regression connects a known external conversation
without a FilmCraft archive, rejects new/other targets, reopens with fresh app
state and confirms the same ID, and preserves project identity/revision/epoch.
Actual packaged controls and owner-conversation reconnect require separate
native acceptance; offline tests do not claim those outcomes.


Persistent Codex session lock and speaking-voice selection passed their complete source-stable
quality gate in `626aad8e` (2026-11-01): 4,311 workspace tests passed, 50 skipped; strict Clippy,
doctests, translation report, native Metal timeout-policy regression, dependency layers, 69 assets
and 49 WASM crates passed. The matching immutable release is building. This is not packaged live
voice-selection, existing-chat Desktop reconnect, or owner-conversation lock acceptance.

The subsequent headphones interruption candidate has seven exact-module packet/mute tests, plus
new integrated regression coverage for persisted mode, hostile parameters, configuration-before-
start ordering, configuration failure containment and preserving explicit mute. The integrated
suite has not yet qualified this candidate. Native acceptance must verify an audible interruption
while the assistant speaks through headphones, continued conversation, and mic-only mute without
replaying muted speech. Never connect the real locked editing conversation to a generated test
project merely to qualify the lock; the owner must choose the intended project first.

## Codex reconnect voice context (2026-11-04)

Conversation identity and realtime recall are separate acceptance requirements. An isolated
signed-in Codex process was stopped/resumed and retained a generated text-only phrase on the
same thread. A separate production-controller WebRTC test used generated speech, no physical
audio devices, no user project/conversation and an MCP fixture that refuses every tool call.
The empty-history comparison learned “copper otter”, then reported it unavailable after a short
voice restart and used a generic editing role. With role-bearing V3 startup history and the
selected-assistant summary, the replacement voice replied “copper otter, concert editor”. The
initial qualified two-cycle test completed in 24.33 seconds. The final current-binary test also
passed with Finder-like PATH (`/usr/bin:/bin:/usr/sbin:/sbin`) in 25.14 seconds, again retaining
the exact phrase and role. A preceding pre-recall data-channel closure is
preserved separately and does not qualify or contradict recall.

The repair also supplies selected cached dialogue after a full Console reconnect, bounds UTF-8
history/identity excerpts, labels partial context, resets cross-thread history and never submits
historical requests as new turns. Synthetic tests cover those boundaries and short restart
protocol ordering. The macOS CLI resolver preserves PATH precedence and falls back to standard
Homebrew locations for Finder launches; it reports the launch error kind instead of hiding it.

This is actual generated-speech voice-transport evidence, not a packaged microphone/UI test or
proof that every earlier utterance fits the bounded handoff. Short packaged Voice Off/On and full
Console disconnect/resume recall/no-replay remain required. The private logs, source-gate result
and generated fixture descriptions are in `target/codex-reconnect-context-20261104/`. Do not infer
completion from the same thread ID alone, and do not enable the deferred API route for these tests.

The C33 final source-stable gate passed 4,400 workspace tests (52 skipped), strict Clippy,
doctests, native Metal timeout-policy regression, layers, 69 attributed assets and 49 WASM crates.
The immutable matching full-feature release was signed and signature-verified. Its exact packaged
CLI initialized MCP and saved a generated project copy without changing the active revision;
no GUI, microphone, owner project or AI request was used by that package smoke. Existing owner
memory/settings hashes and old demo executables were unchanged during preparation. Evidence:
`target/codex-reconnect-package-20261104/qualification-result.json` and
`target/codex-reconnect-package-20261104/headless-smoke-result.json`. These checks do not replace
the pending short packaged voice/full-Console reconnect acceptance.

## Active-turn inactivity recovery candidate (2026-11-04)

The owner's signed C33 demo kept voice active in the locked conversation while worker responses
retried with `responseStreamDisconnected`. A read-only native check showed `contextPending=false`;
no rejected native edit was found after the last successful edit. This is not evidence of lost
edit permission or an exhausted account. The reason for the upstream stream failure remains open.

A separate local watchdog defect has exact owner evidence: a turn began at 00:13:40.397 UTC,
FilmCraft logged its ten-minute timeout at 00:23:40.402 UTC, and the native `edit.duplicate` call
succeeded at 00:23:34.721 UTC. The watchdog used total turn age rather than task inactivity.
The candidate replaces that absolute deadline with a ten-minute progress deadline. Matching task
items, tool progress and text/reasoning deltas count; voice messages, empty deltas, other threads/
turns and retry notices do not. A silent task is interrupted once and stays busy until its actual
completion notification. Missing cancellation acknowledgement is bounded to 30 seconds and reports
an actionable reconnect/inspect error. Historical edits are never replayed.

Synthetic regressions exercise recent tool progress after ten minutes, two simulated hours of
ongoing progress, silent-worker cancellation with retry/voice noise, one interrupt and no replay,
confirmed cancellation followed by a fresh request, and safe structured HTTP status extraction.
Qualification is in progress; no matching packaged or real-provider long-task pass is claimed.
Owner project, session lock, live voice and GUI remain untouched during development. Evidence:
`target/codex-reconnect-context-20261104/owner-turn-lifecycle-review.json`,
`owner-edit-block-recurrence.json` and `owner-context-size-review.json` in the same directory.
