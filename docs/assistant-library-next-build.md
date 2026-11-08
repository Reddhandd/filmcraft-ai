# Assistant Library and memory — next release scope

Owner authorized the complete scope on 2026-11-05. Develop and qualify the entire
set before the next release compilation; focused development/test compilation is
necessary and is not a release. Preserve the installed alpha and all owner data.

## Feature qualification (complete; release acceptance still pending)

- [x] Additive manual assistant updates and autosave, explicit conflict/revision/deletion
  controls, restorable history; follow [memory preservation](assistant-memory-preservation.md).
- [x] Automatic, recoverable memory compaction with protected teaching and unfinished work.
- [x] Rename the reference feature **Assistant Library** and expose **Window → Assistant Library**
  independently of Command Console, keeping one shared data store and the Console shortcut.
- [x] OCR for scanned PDF books, with provenance and honest recognition limitations.
- [x] Broader PDF compatibility, including the supplied Video Editing book; preserve bounds,
  page identity, error reporting, crash isolation and the original document.
- [x] Explicit per-page image requests through MCP; render only requested admitted pages,
  with citations, stale-selection checks, bounded pixels/bytes/time and no retained capture files.
- [x] Larger library capacity using bounded working sets, with measured search/import memory
  and latency. Preserve and qualify migration from the existing index and reference selections.
- [x] Unencrypted EPUB import, indexed chapter/section citations and requested illustrations;
  reject DRM and hostile ZIP/XML inputs without invoking external resources or executing content.

## Release acceptance

Use the original Kodak, DaVinci, Pudovkin and Video Editing references and an authorized
unencrypted EPUB. Test the actual Add Book and assistant update controls in an isolated
native profile, then fresh-process restore and fresh AI-context retrieval. Generated fixtures
cover conflicting/omitted instructions, revision restore, failed compaction, malformed PDFs,
ZIP bombs, traversal, stale page requests and disk-write failure. Private books stay outside
Git and shipped artifacts. Validate all licenses, attribution and portable dependency closure.

Run the full repository gate before each source commit. No requirement is complete solely
because a command exists or a synthetic test passes. Record evidence and remaining limits
per item. Do not start the next release build while any item above remains unqualified.
Additional credentials or a new paid service require an owner decision before enabling them.

## Development evidence — 2026-11-05 (not a release)

The installed alpha remains unchanged. None of the release acceptance boxes above is
closed yet; focused proofs do not substitute for signed native and fresh-context checks.

- Assistant preservation/history: 66 focused tests passed, including partial band-workflow
  updates, explicit review/deletion, autosave, original retention, stale compaction results,
  archive failure and restoration without replaying edits. Compaction has a guarded model
  action and retained-original controls; automatic scheduling/provider qualification and
  protected migration of legacy free-form notes remain open.
- Assistant Library: 13 focused library tests passed, including opening the shared window
  while Console stays closed. An additional EPUB import/reopen/citation regression is now
  being qualified. Existing index schema remains readable through optional metadata defaults.
- EPUB: the isolated parser passed hostile ZIP/XML/truncation tests. A private ZIP snapshot
  of the owner's readable book produced 100 ordered sections and 30,988 extracted text bytes.
  Its original is about 243 MiB compressed, illustrating why the former 64 MiB file limit
  was insufficient. Parser admission is bounded at 256 MiB compressed/512 MiB expanded,
  4,096 entries and 32 MiB per entry; requested XML/text reads have tighter bounds.
  IDPF-obfuscated fonts are ignored, not decrypted; protected text/images and DRM are rejected.
  Directory-packaged EPUB import, requested illustrations and native large-book working-set
  qualification remain open. Larger overall catalog capacity has not yet been approved by
  measurements or implemented.
- OCR: the author's old Hugging Face checkpoint performs poorly on the clear scanned body
  sample, both as RTen and ONNX. The current official ONNX models yield much better readable
  text on that same physical page (about 1.6 seconds including rendering/loading in the
  optimized development probe). More samples and retrieval/error qualification remain open.
  The author explicitly clarified CC BY-SA 4.0 for the model artifacts in
  [the model licensing issue](https://github.com/robertknight/ocrs-models/issues/34);
  the code license alone is not the asset license. Shipping integration/attribution is pending.
- The current renderer independently opens the supplied 70-page Video Editing reference.
  Strict-parser recovery, bounded image delivery and original-file compatibility tests remain
  open. No page image has been delivered to a live assistant by this new feature yet.

Ignored private evidence is under `target/assistant-library-readiness-20261105/` and the
focused test logs under `target/`. Private books/model probes must not enter source commits
or release payloads. No additional AI service, API billing or owner conversation was used.

## Development evidence — 2026-11-06 (not a release)

- The current focused store suite passes 18 tests. It covers disk-index migration,
  damaged-index recovery, explicit missing-index recovery, enabled states, removed
  identities, stale image requests, original-file integrity, EPUB chapter retrieval,
  unpacked EPUB snapshots and rejection of package symlinks. These are source-level
  checks, not native Add Book acceptance.
- Unpacked EPUB directories now use the same bounded archive parser and retained
  original as ZIP EPUBs. The native window includes an Add EPUB Folder control.
  Actual Apple Books package import and native interaction remain to be qualified.
- The application all-target compile check passed before the latest native controls;
  that incremental check is being repeated for those changes. A full 413-page
  Pudovkin OCR import is running against a disposable library with verified local
  model files. That run subsequently passed, as recorded below. No owner data or installed alpha
  has been modified, and no book content has been sent to an AI service.

All release acceptance items above remain open pending their complete evidence.

### Additional private qualification

- The production disk-index modules migrated 512 synthetic books / 131,072 passages
  / 209,977,344 text bytes in 10,532 ms. Reopen plus selected-book search returned
  five hits in 3,755 ms; a separate fresh-process run took 4,339 ms and peaked at
  55,771,136 resident bytes, measured by macOS `time -l`. These are individual local
  runs, not latency guarantees or a full workload distribution. Index cache is
  bounded at 32 MiB. Capacity-bound mutation and corrupt-counter tests pass in the
  19-test focused library suite.
- The real Apple Books directory package imported as 100 sections in a disposable
  library. A subsequent process returned three mixing-related excerpts with chapter
  and section citations and no invented PDF page numbers.
- The actual installer downloaded and checksum-verified 12,212,656 bytes of public
  OCR models, retained their attribution/license locally, and reported zero provider
  requests. No private document was uploaded. The supplied Video Editing PDF is
  being imported with those installed models; full-book acceptance remains pending.

- Full Pudovkin OCR import completed: 413 physical pages, 433 indexed passages.
  Fresh processes retrieved camera, montage, director and aeroplane excerpts with
  physical-page citations and explicit local-OCR provenance. This establishes local
  extraction/retrieval, not perfect transcription quality on every page.
- Video Editing completed all 70 pages with checked recovered structure and local
  OCR provenance. A requested page image was rendered at 1280×720 in 0.288 seconds,
  encoded as 51,698 PNG bytes, delivered inline and discarded; no capture file appeared.
- The focused suite now passes 20 tests, including failed-publication retention of
  the original book and recovery record. A failed commit must never cause cleanup
  to delete an original that an ambiguously committed database could reference.

- The application-level library suite passed 26 tests, including independent Window
  menu access, project/assistant selection filtering and native job boundaries.
  The broader Console suite and strict lint checks are in progress; final signed
  native Add Book/update controls and fresh AI-context acceptance are still pending.

## Native acceptance evidence — 2026-11-06

The signed development-only **FilmCraft Assistant Library Check** used a disposable
profile and generated band project. The installed alpha and owner project/library/
assistant/session-lock data were not changed. AI and microphones stayed disconnected
through these checks. Evidence is private under the ignored readiness directory.

- Actual Save Assistant, Update Assistant, teaching review and opt-in autosave controls
  preserved the band synchronization procedure while incorporating reviewed teaching.
  The version-preview/restore control restored earlier teaching without replaying any
  project edit. A second generated procedure persisted through close-time autosave flush.
  Fresh-process project/assistant loading restored both procedures and all six books.
- Synthetic native compaction condensed 96 decisions to one, retained a 13,914-byte
  original, and preserved teaching and unfinished tasks byte-for-byte. Explicit original
  restoration recovered all 96 decisions without changing the project file. This proves
  host adoption/restoration, not model-authored summary quality or provider qualification.
- Actual Add Book/EPUB Folder pickers imported Kodak (216 physical pages), DaVinci
  Parts 1/2 (330/329), Pudovkin (413, with local OCR), Video Editing (70, checked parser
  recovery and local OCR), and the authorized unpacked EPUB (100 sections). The native
  OCR installer downloaded and verified the pinned models without sending book data.
  Kodak was first imported before installing OCR and honestly reports its 17 unprocessed
  text-empty pages; this is not a claim that every physical page has searchable prose.
- Assistant Library remained usable with Command Console closed. Selected-book and
  stale-generation guards rejected inadmissible images. Native PDF-page and EPUB raster
  illustration delivery produced bounded inline PNGs with physical-page/chapter citations;
  image results were consumed once and no capture file was retained. Raster EPUB delivery
  currently supports PNG/JPEG; unsupported illustration formats return an explicit error.
- The complete repository gate passed on unchanged source before the final launch-only
  isolation adjustment: 4,472 passed, 52 skipped; layers, 69 attributed assets, and 49-crate
  WASM check passed. Its first sandboxed run had localhost permission failures; the completed
  permitted run passed. No failing test was waived. The new session-policy override unit
  test passes, but the full gate must run again against the final source.

Still required: fresh live-AI context/retrieval and supported-provider compaction proof,
then final source gate, Apple Silicon release packaging/dependency/license closure and
signed release acceptance. The launch-only `FILMCRAFT_SESSION_LOCK_FILE` test override
keeps the default cross-build policy intact while allowing private model qualification.

### Live-AI qualification and final boundary repairs

A fresh isolated Codex conversation reported both saved procedures, retrieved cited
Kodak exposure guidance from physical page 54, requested the page and described its
visible density/log-exposure graph, exposure-index section and film-speed table.
The private provider trace contains actual pixel-bearing image blocks; the generated
project revision remained unchanged. No owner conversation, microphone or new paid
API service was used. Local cleanup does not assert provider-side image deletion.

This test first exposed a final MCP response-budget error: native library rendering
worked, but the generic reference-tool text limit omitted the image. The wrapper now
admits one bounded typed library image while preserving the smaller text budget and
rejecting oversized metadata/images. A real-PNG final-transport regression is included.

The automatic compaction trace then exposed a distinct history/semantic-version mixup.
Its prompt now explicitly obtains history at offset zero and keeps the two version
namespaces separate. Summary adoption requires the exact full-history generation as
well as semantic scope/epoch/version. A 74-test assistant run passed, including rejection
of stale history when the semantic version is unchanged. The corrected provider path
still requires its live retest. Maintenance that finishes without a usable summary now
reports that outcome rather than silently appearing successful.

The gate completed successfully while these boundary repairs changed source, so its
source-stability check correctly rejected that run as release evidence. A final stable
source gate remains required before any commit or release compilation.

### Final feature-qualification results

- The corrected automatic host schedule completed real Codex summarization in 34.8
  seconds: 96 generated decisions became two; the 13,861-byte original was retained.
  Teaching and unfinished tasks matched exactly, project revision was unchanged, and
  the model explicitly preserved uncertainty about historical assertions versus actual
  timeline verification. No manual compaction prompt, microphone or owner session was used.
- The focused assistant suite passed 74 tests. The final MCP transport test also passed
  with a real PNG larger than the text budget and its physical-page citation intact.
- Production index import of 512 generated books / 131,072 passages / 209,977,344 text
  bytes took 11,229 ms, peaking at 63,291,392 resident bytes (60.4 MiB), with a 32 MiB
  index cache. The earlier separate query process peaked at 53.2 MiB. These measurements
  cover index ingestion/query, not an entire editor/video-decoding working set.
- A generated old-schema library was migrated through the signed native app: identity
  9 and generation 7 were preserved, legacy index/original hashes were unchanged, search
  worked, and the saved assistant retained its explicit book selection. No owner profile
  was migrated during qualification.

All eight feature rows now have source, regression and relevant native/live evidence.
This does **not** complete the goal: the final stable-source gate, source commit,
Apple Silicon release build, dependency/license inspection, and relocated signed-package
acceptance remain outstanding. The installed alpha is still unchanged.
