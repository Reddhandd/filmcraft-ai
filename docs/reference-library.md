# Persistent local Reference Library

The library stores user-imported PDF, UTF-8 TXT and MD references in the preserved application
profile under `Console/ReferenceLibrary`. It is separate from project memory, reusable assistant
instructions and one-shot attachments. Imported books and extracted text are private user data;
they must never be committed, embedded in a build or included in release assets.

Open **Reference Library** from the Console's folder icon. **Add Book** imports one selected
file at a time on a bounded worker. Search returns a few excerpts with document identity, filename
and physical PDF page number. This first implementation uses a local word index, not embeddings,
model training or full-book context injection. A fresh conversation can use the existing index.
Only the requested catalog or excerpts cross the AI connection; the whole book is not uploaded.
Book text is quoted reference data, never instructions, permissions or executable commands.

A checkbox controls whether a book can be retrieved from the shared library. **Remove** removes
its searchable content and retained original, without deleting the file originally selected by
the user. A permanent removal ledger prevents older backup indexes from resurrecting deleted
passages. Failed original cleanup is reported separately. A source candidate adds project and saved-assistant subsets; it still requires its full gate
and matching packaged/native acceptance. The signed `c0a6f734` package retains shared-library
behavior until that candidate is delivered.

## AI retrieval

Use the existing `console_references` tool:

- `console.library.inspect`: optional `offset`, `limit` (1–16), `expectedGeneration`.
  Follow `nextOffset` with the returned generation guard; restart at zero if it changes.
- `console.library.search`: `query` (1–256 UTF-8 bytes, 1–16 words), optional `limit` (1–5).
  Results include document title, source filename, physical page, passage identity and text.
- `console.library.status`: poll the returned `ticket` until complete. Do not enqueue the
  same operation repeatedly while pending.

Models cannot import arbitrary filesystem paths, enable, delete or recover books through this
tool. Those operations are explicit native controls. Retrieval does not edit projects, change
session locks, start audio or replay prior edits. The tool already exists, so this feature does
not require a new MCP tool name, but the editor and helper must be from the matching new build.

## Storage and failure handling

Schema 1 uses atomic `index.json` publication and `index.previous.json`, an OS writer lock,
numbered original copies, and `removed.json`. Missing or malformed current indexes can expose a
previous catalog with a warning; searching stays disabled until the user chooses **Restore
Previous Index**. Recovery preserves permanent removals. Both invalid indexes produce an error
while retaining original books. Original publication never overwrites an existing identity, including an unindexed original
left by recovery. Failure to write the original or index preserves previously
published content; errors do not mutate projects. Global enable/disable and removals rebuild
postings. Updating a book currently means removing the old entry and importing the revised file.

Limits: 128 retained books, 64 MiB original, 8 MiB extracted text per book, 4,096 physical pages,
32,768 passages and 64 MiB serialized index. Passage text is at most 1,600 bytes with overlap.
PDF parsing runs in the existing isolated child with structure/decompression budgets, per-call
deadlines and a two-minute indexing deadline. Background UI admission is bounded; a timeout does
not start duplicate workers. Encrypted, malformed, unsupported or image-only PDFs can fail with
an actionable error. No OCR or PDF image/layout inspection is claimed.

The local maintenance example `reference_library` supports importing a separately prepared,
page-preserving extraction of an explicitly selected original. Its `importPrepared` operation
is not exposed to models or the normal file picker. The owner-authorized 2026-11-05 import used
installed Poppler for four PDFs the built-in extractor could not handle; Poppler is not linked,
bundled or required by FilmCraft. Reimporting those originals through the native PDF picker is
not yet qualified. The imported text and original bytes remain local on D.

## Recorded qualification

Eight owner-selected books cover 1,665 physical pages and 2,367 passages. Each retained original
has the same SHA-256 as its source in Downloads. Four representative searches, each in a fresh
process, completed in about 28–30 ms and returned cited passages. These measurements qualify
this local corpus and machine only, not semantic search quality on all books. No connected
model interpretation or native-window acceptance is claimed by these measurements.

Private evidence: `target/reference-library-20261105/owner-index-qualification.json`,
`import-result.json`, and `prepared-import-result.json`. Synthetic regressions cover fresh-editor
retrieval, malformed indexes, Unicode boundaries, duplicate import, disabled/deleted references,
backup recovery, occupied-original preservation, truncation/bit-flip mutations and hostile parameters. Packaging and native/MCP
acceptance remain separate gates.

## Project selections (source candidate)

Choose **Use all available books** to follow shared availability, or **Choose books for this
project**, then check **This project** beside the desired books. An explicit empty selection
returns no passages. Shared availability remains a separate control; a globally disabled book
cannot be retrieved through a project selection. Missing selected books report an error rather
than falling back to unrelated references. Selection uses the stable identities of this local
library; it does not discover or substitute books in a different library/profile automatically.

Selections are retained in project assistant checkpoints. **Save/Update Assistant** carries the
current selection with that named assistant; loading it into another project restores its
selection without importing the source project's history. Existing checkpoints and assistants
use shared availability. Instruction autosave preserves the catalog's reference selection;
explicitly Save/Update Assistant when changing its reusable book choices. Stale reference-choice
updates are rejected even if the instruction text is unchanged.

The native-only `console.library.selection` command accepts `documentIds`: `null` for shared
availability or an array of at most 128 unique positive identities. Models cannot set this or
inject selection overrides into search parameters. Retrieval filters before ranking and rejects
pending or cached search results after the project or selection changes. Memory-load failures
must not silently switch retrieval to shared availability. No selection action edits the timeline,
replays commands, starts voice, or changes a Codex connection.
