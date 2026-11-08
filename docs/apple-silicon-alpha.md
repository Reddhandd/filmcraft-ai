# FilmCraft AI: Apple Silicon Mac alpha

Status: independent public-alpha candidate; publication status is shown in this fork’s GitHub Releases. This distribution targets native arm64
Macs. Windows, Linux and Intel Mac validation are outside this alpha's release scope.

This is an independent modified build based on FilmCraft by the ArtCraft team, not an official
ArtCraft release. The alpha feature excludes the ArtCraft wordmark bytes and official-family
presentation while retaining source, author and licence attribution. This fork removes upstream trademark artwork from its current source tree. See `docs/brand/LICENSE-brand.txt`.

## Release requirements

- A relocated app opens through Finder without this checkout, a developer launcher, an existing
  control-token file, or an owner-specific data path. Include the matching `filmcraft-cli` MCP
  helper inside the app. External Codex remains a separately installed and authenticated service.
- Ordinary desktop startup creates a fresh private credential and binds an OS-assigned loopback
  port. Startup needs no credential file. Credentials are not shipped, logged or added to
  command-line arguments; provider children receive them through their private environment.
  When explicitly connecting Desktop, its existing owner-only setup files carry the local
  bridge credentials for the external MCP helper. Explicit developer control
  launches retain their configured-port and environment-token behavior.
- Preserve normal per-user preferences, projects, recovery, Undo and assistant storage. Distribute
  no personal books, assistant profiles, conversation IDs, credentials or recovery files.
- Verify arm64 architecture, deployment metadata, system-library dependencies, signatures and
  archive contents. A declared minimum OS version is not proof of runtime validation there.
- Exercise the relocated package with generated projects and an isolated data profile: launch,
  save/reopen, export, Console/helper setup, library and assistant persistence. Record actual
  provider and attended test scope separately. Do not equate an isolated profile on one Mac
  with testing a second physical Mac or an older macOS release.
- Label ad-hoc versus Developer ID signing and notarization accurately. Do not claim Gatekeeper
  acceptance on another Mac without corresponding evidence.
- Prevent this alpha from being silently replaced by an unrelated upstream release stream.
  Review the update channel before distribution.

## Current acceptance

The owner accepted assistant stability after heavy frame-by-frame analysis and attended timeout
recovery restored voice, working context and editing. Exact Codex conversation identity is now
optional: assistant memory, control instructions and relevant library access must load before
useful work resumes. Keep unexpected disconnect diagnostics and any recurrence actionable.

Persistent library controls, per-project and saved-assistant book subsets, live cited retrieval
from an original supplied book, and named-assistant autosave/restore passed matching C40 native
checks. These do not qualify every document or new audio-understanding capability.

FilmCraft's disk-free screenshot delivery passed local checks. Provider conversation image
retention remains a separate unresolved requirement; do not describe local file cleanup as
provider-side deletion. The latest Desktop wrong-session report also needs matching acceptance.

The reported export-audio concern was withdrawn: the measured approximately 1 dB attenuation
matches the owner's intentional gain change. It is not an established export defect.

## Deferred work

Direct API connections remain disabled and unresolved. Original-audio delivery and understanding
remain a future qualified milestone. Premiere import, other operating systems and Intel support
are not release blockers for this Apple Silicon package. Previously accepted playback stress-case
and export-estimate accuracy follow-ups remain deferred.

Publishing requires review of the intended repository, license/attribution contents and release
artifacts. Building a package does not publish it or authorize changing an upstream release.

## Reproducible package command

Installation and provider setup are in [the alpha installation guide](apple-silicon-alpha-install.md).

Build on Apple Silicon with the Rust target and macOS command-line tools installed:

```sh
packaging/macos/package.sh --arch aarch64 --ai-alpha
```

The `ai-alpha` feature includes the existing optional Whisper and neural-voice features and
suppresses automatic startup checks against the upstream editor release channel. It does not
bundle downloaded speech models or re-enable direct API connections. Manual upstream updates
are not a supported update path for this alpha; install a reviewed FilmCraft AI release instead.
The alpha package builds both the editor and its full-feature MCP helper; `--skip-build` is
rejected to avoid relabeling a binary from another feature configuration. On a native Apple
Silicon build it reuses the host release cache instead of duplicating an arm64 target cache.
Signing defaults to ad-hoc unless a distribution identity is explicitly configured. See the
packaging script for notarization inputs; never put credentials in source or release archives.
