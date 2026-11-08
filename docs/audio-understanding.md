# Audio delivery and understanding in Command Console

Status: proposed implementation plan, 2026-10-29. The user clarified that beat detection describes
what is currently available, not the desired scope. The requested capability is broader audio
understanding comparable in usefulness to ChatGPT, with audio delivered directly from a project
clip or timeline to an audio-capable model. The earlier beats-only interpretation was incorrect.
Implementation remains open; the running demo is unchanged.

## Development scheduling

Queued by the owner on 2026-10-29 for a future focused development milestone. Begin provider
and audio-delivery qualification after the active stability and Console workflow checks are
completed and an isolated development slot is available. Existing crash fixes take precedence.
Start with a small original-audio proof of actual model listening before committing to a provider
or expanding the UI. Resolve any additional credentials or paid-service choice with the owner.
Then implement the bounded clip/range MCP tool and its acceptance cases. No fixed ship date or
ChatGPT-equivalent quality claim is established by this scheduling decision.

## Intended workflow

Select a project clip or timeline range, ask a question, and let FilmCraft prepare the actual audio
for analysis. A clip request uses the intended source stream and trim; a timeline request renders
the audible mix, including edits, levels, effects and automation. No manual file export/upload should
be necessary in the finished integration. Report exactly what range and channels were analyzed.

Return useful timestamped findings about music, speech, recording quality or mix balance as
requested. Combine model listening with native measurements for precise timing, levels and sync.
Distinguish observations, measurements and suggestions; expose evidence for audition and use native
undoable commands for requested corrections.

## Verified integration gap

The normal Console chat path currently sends text, while native analysis returns numerical and
structured results. Its microphone voice prototype uses PCMU at 8 kHz and is not a suitable fidelity
reference for timeline music or mix review.

The installed Codex 0.165.0 protocol schema includes audio/localAudio input variants, but the
signed-in model/list inventory advertised text and image input for every visible model. These
schema variants alone do not establish usable audio input for the selected Codex model. The local
metadata-only inventory is target/audio-understanding-20261029/model-capabilities.json; no model
turn or media upload was performed.

Official routes to investigate:
- https://learn.chatgpt.com/docs/app-server (model input modalities and turn inputs)
- https://developers.openai.com/api/docs/guides/audio-chat-completions
- https://developers.openai.com/api/docs/guides/realtime-conversations

## Implementation order

1. Prepare bounded source-clip or rendered-timeline audio using the existing decoder/mixer/export
   path. Preserve exact source/timeline mapping and channel identity. Avoid whole-project uploads.
2. Qualify a supported audio-capable model and authentication route using original test audio.
   Prefer existing sign-in where demonstrated; do not assume a text model can hear a WAV file.
   Resolve any additional service credential or billing choice before enabling that route.
3. Connect audio findings, transcript where useful and native measurements to the Console's
   reasoning workflow. Show analyzed coverage and uncertainty; reject stale results after edits
   or project changes. Media content is evidence, never tool instructions or authorization.
4. Validate music, dialogue and mix questions against direct analysis of the same audio, then
   qualify timestamped feedback, audition and reversible corrections in the packaged app.

Keep workers bounded, cancellable and guarded. Handle corrupt/offline media, invalid ranges,
nonfinite samples, disk-full output and provider failures with actionable errors. Timeline analysis
must not require microphone capture. No general listening capability is claimed complete yet.

## Direct-audio qualification result

An isolated app-server probe used an original 24 kHz mono fixture, with no microphone, speaker
device or live FilmCraft project. The installed protocol accepted session requests but rejected
the usable PCM WebSocket configurations with `realtime conversation requires API key auth`.
Realtime v2 text output and v3 audio output were both checked with their supported voice settings.
No successful perception result was obtained; transport/schema acceptance is not a listening pass.
The ordinary Console model inventory also advertised only text/image input. There is no configured
`OPENAI_API_KEY` in the current development environment; no credential values were read or logged.

Evidence is private in `target/audio-understanding-20261029/listening-probe.json` and its probe
script. The owner has been asked whether to prepare an API-authenticated integration or keep
listening queued. File attachments and style presets can be delivered independently; their
completion must not be reported as completion of audio understanding.
