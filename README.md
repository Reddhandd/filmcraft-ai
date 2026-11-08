# FilmCraft AI

Independent Apple Silicon Mac alpha based on [FilmCraft](https://github.com/storytold/filmcraft)
by the ArtCraft team and community. This fork is not an official ArtCraft release and is not
endorsed or supported by the upstream team.

## Download

Get the DMG from [GitHub Releases](https://github.com/Reddhandd/filmcraft-ai/releases).
The first public package is **0.13.0-ai-alpha.3, installer revision 1**, for Apple Silicon Macs.
It is ad-hoc signed and **not notarized**. macOS may block opening it. Read the
[installation and provider guide](docs/apple-silicon-alpha-install.md) before testing.
Intel Macs, Windows and Linux have not been qualified for this release.
The declared minimum is macOS 11; older supported macOS versions and other physical Macs
have not been independently tested. Do not interpret a deployment target as runtime qualification.

## Included workflows

- Embedded Codex text and voice with FilmCraft inspection and editing tools.
- External ChatGPT Desktop MCP connection; one editing provider selected at a time.
- Named Project Assistants, additive teaching updates, opt-in autosave and recoverable memory compaction.
- Assistant Library with indexed local references, cited passage retrieval, scanned-PDF OCR,
  unencrypted EPUB text and requested page/illustration images.
- Program, Source and Timeline inspection, native Undo and reconnect recovery.

Books, assistants and memory are references and saved instructions, not model training.
No AI account, Codex executable, personal books, conversations, projects, media or credentials
are included. Install and authenticate your own compatible Codex CLI or configure your external
MCP-capable ChatGPT client. Provider availability, usage limits and client support can change.

## Limits and privacy

Direct API connections are disabled and unresolved. Original-audio music/mix understanding
is not qualified. The app retains no internal capture files, but provider conversation history
may retain sent images; local cleanup is not provider-side deletion. Document compatibility,
OCR recognition and moving-image playback have workload-dependent limits. Some valid EPUBs
with XML DTD declarations are rejected; DRM-protected documents are unsupported.

Back up projects and application data before testing. Different FilmCraft builds may share
per-user storage; do not run multiple builds against the same profile concurrently. Upstream
automatic update checks are suppressed for the AI alpha; use reviewed releases from this fork.

## Build from source

Requires Rust and macOS command-line tools on Apple Silicon. Existing dependency versions are
pinned in `Cargo.lock`. From this checkout:

```sh
cargo build --release --locked -p filmcraft -p filmcraft-cli --features filmcraft/ai-alpha,filmcraft-cli/whisper
packaging/macos/package.sh --arch aarch64 --ai-alpha
```

See [architecture](docs/architecture.md), [agent controls](docs/agents.md),
[contributing](CONTRIBUTING.md), and [release provenance](docs/public-alpha-release.md).
Historical roadmap documents describe upstream plans and earlier development snapshots;
they are not claims that every listed feature is available or that other platforms were tested.

## Licence and credits

Source is licensed **MIT OR Apache-2.0**; preserve [NOTICE](NOTICE), the licence texts and
[asset/dependency attribution](ATTRIBUTION.md). Third-party assets and OCR model downloads
have their own notices. Upstream trademark artwork is removed from this fork's current tree
and independent release. Historical upstream commits retain their original provenance.

Report independent-fork problems in [this repository's issue tracker](https://github.com/Reddhandd/filmcraft-ai/issues).
Please do not assume upstream maintainers support this modified build.
