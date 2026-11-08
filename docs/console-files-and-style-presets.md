# Console attachments and creative presets

Status: initial implementation and local acceptance, 2026-10-29. Add Files/drop, bounded
text/PDF/PNG references, reusable style library, review-before-save chat drafting, project
selection and import/export are implemented. See [supported inputs and limits](console-reference-inputs.md)
and [qualification](testing.md#console-references-and-styles-2026-10-29). The requirements below
remain the design intent; audio/video listening and dedicated project/range pickers remain open.

## Attach files to a Console message

Provide an Add Files button and drag-and-drop onto the Console, with visible attachment chips,
remove controls, processing status and actionable unsupported-format errors. Support picking
files from disk and referencing existing project items or explicit timeline ranges. Attaching a
file for discussion must not silently import it into the timeline.

Start with documents/text and images, using bounded extraction and supported model inputs.
Audio/video attachments share the capability routing and clip/range preparation work in
[audio understanding](audio-understanding.md). A file being readable does not mean the selected
model can hear audio or watch video. State the actual coverage: extracted text, inspected pages,
sampled frames, transcript, native measurements or audio-model listening.

Keep source identity and page/time references with findings. Handle missing/moved files, duplicate
names, corrupt inputs, size/page/duration limits and cancellation. Define bounded per-message
attachment counts and byte budgets before implementation. Make any persistence behavior visible;
save attachment metadata with the correct conversation/project and never silently attach files
from a previous project. Treat document/media contents as reference data rather than tool
instructions. Reference attachments are not licensed product assets or permission to redistribute.

Expose attachment inspection and supported add/remove/send operations through versioned native
commands and MCP, with hostile-parameter tests and the same validation as the GUI.

## Selectable creative memory presets

Build on existing project memory with named, editable presets that the user can create, duplicate,
rename, delete and select. Initial scope: one active preset per project, saved durably; reusable
presets live in a user library. A preset selection is visible in Console and can be disabled.

### Save a style developed in chat, then load it later

Users can work out a style conversationally, then ask "Save this as a style preset" or choose
Save as Style Preset. Codex drafts a self-contained style description from the agreed creative
choices and shows it for review and editing, with a user-chosen name, before saving. Preserve
the user's own wording where requested. Do not save the whole transcript or silently include
unrelated project facts, private file paths, temporary instructions or permissions to edit.

Provide Load Style Preset to browse the saved library, preview the description and load it into
the current project/conversation. A preset must work in a fresh conversation without access to
the chat that produced it. Users can also write or paste a description directly, or import/export
a versioned preset file to move it between projects or installations. Treat imported descriptions
as creative preferences, not executable commands or authority to change safety rules.

After discussing refinements, offer Save Changes or Save as New Preset; do not silently overwrite
the reusable original. Track the selected preset/version and any project-specific overrides so
loading, switching, disabling and resuming produce clear, current context. Expose save/load through the same native command and MCP validation paths as the UI.
Keep arbitrary-path import/export as native user controls.

Example preferences, editable rather than claimed universal rules:

- Restrained documentary: prioritize dialogue, continuity and natural pacing; use few effects.
- Energetic music edit: favor musical phrases and selected accents, alternate coverage, preserve
  occasional held shots and avoid mechanically cutting on every beat.
- Performance fidelity: preserve complete phrases, source sync and natural timing; avoid retiming
  unless explicitly requested.

A preset can describe pacing, typical shot duration preferences, transition restraint, effects,
color intent, music/dialogue balance and things to avoid. It guides choices when the user asks
for creative work; selecting it does not immediately change any clip, effect or timeline state.
Applying a concrete look or edit remains a separate native, undoable operation.

Resolve conflicts clearly: the current explicit request overrides project-specific creative
decisions, which override the selected preset's defaults. None overrides project safety rules,
available capabilities or factual evidence. Show the effective combined context and refresh it
when presets change; an old conversation must not retain a silently conflicting preset.

## Acceptance

Verify attachment selection/removal, supported processing, coverage/provenance, bounded failures,
save/reopen and project switching without context leakage. Audio tests must demonstrate actual
listening before claiming it. Verify preset CRUD, durable project selection, effective-context
inspection and refresh on resume/switch. With a controlled editing task, different presets should
lead to explainably different choices while an explicit user instruction takes precedence.
Verify that a style agreed in chat can be reviewed, named, saved, exported/imported and loaded in
a fresh conversation/project without the original transcript. Test malformed/oversized preset
files, name collisions and failed writes without losing an existing preset or its active selection.
Selecting a preset or attaching a reference alone must leave the timeline and Undo history intact.

## Project assistants

**Project assistants** selects reusable role, goal, preferences, and workflow instructions.
**Save Assistant…** asks for a name. Replacing an existing name requires the displayed
version to still match the saved library; concurrent changes produce an error.
**Creative preferences** exposes the existing style presets and style editor. Existing
preset files remain available. Saving an assistant includes its selected style description.

Each project keeps its own structured assistant checkpoint beside the project, in its
`.console` directory. Changed context is saved every 15 minutes, after a completed AI
turn, and before a clean disconnect or project close. The status shows the last successful
save. A failed write retains the previous checkpoint and reports an error; disk writes are
bounded and run on a worker. A previous valid checkpoint can recover a corrupt current one. A recovery notice identifies when older
assistant notes were restored. The checkpoint timer keeps running with the Console closed;
a clean, idle assistant does not request continual update passes. If neither checkpoint is
readable, FilmCraft reports that assistant memory is unavailable and keeps the existing
conversation/project notes usable. Unloaded, unreadable assistant files do not prevent normal
project close; failures saving already-loaded changes still protect those changes.

Checkpoints include learned instructions, decisions, pending work, recent conversation
excerpts, and a bounded command-outcome ledger. Command records are historical data and
are never executed when memory is restored. Large command parameters are explicitly
omitted; credential fields are redacted. Named assistants contain reusable instructions,
not another project's conversation or command history.

Agents can use `console_memory` actions `assistant_read`, `assistant_update`, and
`assistant_history`. Updates require the current scope, document epoch, and memory version.
The structured update accepts instructions, decisions, pending work, and errors; the host
owns command outcomes and project identity. Automatic checkpoints preserve recorded
context; semantic summaries depend on the assistant recording what it learned.
