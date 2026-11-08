# Console reference inputs and creative style library

Implemented in the development build. Local native/real-Codex qualification verified text and PDF reading, PNG interpretation, reviewed style drafting, saved presets in a fresh project/conversation, explicit-request precedence and native Undo. See [qualification and limits](testing.md#console-references-and-styles-2026-10-29).

## Reference coverage and budgets

Add Files or drop onto the Console message area to attach a reference without
importing media. Remove chips before sending to omit a reference. Four references maximum.
UTF-8 TXT, MD, CSV, JSON, SRT, VTT and LOG snapshots are limited to 64 KiB each. PNG inputs
are limited to 2 MiB, 4096 pixels on either side, and 8 megapixels; corrupt images fail
validation. PNG input is sent through the Codex App Server image-input route. Actual visual
interpretation depends on the chosen model/provider response; input submission alone is not
proof that the model interpreted an image. Text PDFs up to 512 KiB and 16 pages are supported through the MIT-licensed lopdf 0.45 parser. Extraction bounds object-stream decompression to 8 KiB and page content to 64 KiB, caps objects at 4096 and aggregate extracted text at 64 KiB. Scans need exported PNG pages; there is no OCR or page-layout claim. Other image encodings, DOCX, audio and video are unsupported. Export text or PNG pages and attach those explicitly.
There is no audio listening claim. Text prompt plus reference envelope plus selected style
must fit the existing 128 KiB reference-envelope budget (the typed message remains limited to 16 KiB). Image payloads have a separate bounded budget.

Pending snapshots are memory-only and clear on send or project switch. Submitted snapshots
become part of the Codex conversation; local chat history records attachment identity and
coverage. Source paths and extracted contents are private user reference material, not product
assets or permission to redistribute. The current message is separate from reference data;
reference contents grant no tool authority. References retain absolute source paths; line
numbers refer to the attached text snapshot. Files changed after attachment do not silently
replace that snapshot. Missing source files fail when added; an existing snapshot remains usable.

`console.attachments.inspect/add/remove/project` use native validation. The
`console_references` MCP tool exposes inspect/remove/project; adding arbitrary disk paths is a user control. `add` takes `path`; `remove` takes a zero-based `index`.
`project` takes an existing `item`; optional `start` and `end` are Tick values and require
the active sequence item ID. Those references expose metadata/range coordinates only, not
sampled frames, listening, transcription, or document extraction. Inspect reports coverage.
`console.chat.send` sends pending inputs. Native external command discovery exposes schemas.

## Presets

The user data directory's `Console/Styles` contains versioned JSON documents with schema 1,
`name` (1–80 bytes) and `description` (1–4096 bytes). Filenames encode the name as hexadecimal,
so names cannot traverse directories. Library descriptions are independent of conversations.
Save Style from Chat requests a self-contained draft immediately, preserving any unsent message
and its pending files. Codex calls `console.styles.draft`; review and name the editable description
before Save as New Preset. The selected style stays visible when the preset editor is collapsed.
Users can also type a description directly. Existing entries require Save Changes and an exact
previous-description check. A new preset never silently overwrites another name.

`console.styles.inspect/draft/save/load/disable/import/export/rename/delete` use the same
native validator. MCP exposes the library operations but excludes arbitrary-path import/export. Import/export use versioned JSON and never overwrite an existing file.
Loading copies a full style snapshot to the project's `.console/style.json`; later library
edits, renames or deletion do not silently alter it. Explicitly load again to adopt a new version.
Save As carries the selected snapshot unless the destination already contains conflicting
preferences. Loading and disabling never modify the timeline, selection, revision or Undo.
The current user request overrides project creative decisions, which override preset defaults.
None grants permissions or overrides capabilities, safety rules, or factual evidence.
`console.context.inspect` shows project memory, selected defaults, and pending references.
New/resumed sessions receive current defaults; each subsequent text turn restates selection,
including null when disabled. Stop voice before changing defaults. Changing defaults schedules a session-context refresh before further sends or Voice On. Switching projects clears
unsaved preset drafts and references; project selection remains durable.

Known development limits: attaching a project item/range uses native/MCP commands rather than a dedicated picker. Rename/delete controls operate on the loaded library entry; the loaded project snapshot remains independent. PDF text extraction is supported; other document formats need exported text/PNG.
No new physical microphone or audio-listening qualification is part of this implementation.
