# Assistant memory preservation requirements

Status: implementation authorized by the owner on 2026-11-05 as part of the
[complete next-build scope](assistant-library-next-build.md). The installed alpha’s Update Assistant and autosave replace instruction snapshots.
The unshipped implementation adds preservation and review, but the complete next-build
acceptance checklist remains open.

## Additive updates

- Preserve saved teaching when it is absent from a newer session or partial update.
  Add genuinely new preferences and procedures; avoid duplicate entries.
- Give reusable teaching stable identities. Preserve the assistant identity and its
  existing reference selections unless the user explicitly changes them.
- Present contradictory teaching for review. Until resolved, retain the last approved
  instruction and keep the proposed change pending; autosave must not pick a winner.
- Forget or revise teaching only through an explicit user action. Distinguish updating
  an existing assistant from creating another assistant or intentionally replacing all
  instructions. Do not resurrect forgotten teaching from summaries or old checkpoints.
- Apply the same preservation rules to manual updates and opt-in autosave. Preserve
  concurrency guards and atomic writes; show whether a save succeeded or remains pending.
- Keep reviewable, restorable revisions. Bound retained history with a disclosed retention
  policy; restoring history must be explicit and must not replay project-edit commands.

## Automatic memory compaction

- Trigger before a measured working-memory budget is exhausted, with headroom for a
  pending update. Set the threshold from actual encoded size and delivery constraints;
  bytes are not a substitute for provider token counts. Do not claim a larger model context.
- Separate protected teaching from compressible session notes. Keep approved standing
  instructions, learned procedures and preferences intact; keep unfinished work explicit.
  Condense repetitive discussion and completed work, preserving useful decisions,
  provenance and uncertainty. Summaries are data, never permission to execute edits.
- History and semantic context use distinct version domains. Start `assistant_history`
  at offset zero without a semantic-generation guard; continue using its returned
  `memoryGeneration`. Submit compaction with that history generation plus the same
  snapshot's `contextGeneration` as the semantic guard. A changed full history must
  reject compaction even when standing teaching has not changed.
- Stage compaction against a versioned snapshot. Validate schema, bounds and protected
  entry identities/content before atomically adopting it; reject stale or incomplete output.
  Preserve a recoverable original. Interrupted work, full disk or provider failure must
  leave the last valid memory usable and report an actionable status.
- If protected teaching itself exceeds capacity, report that and offer organization or
  explicit user review. Never silently drop teaching to make a summary fit.
- Any model-assisted summarization must use an explicitly supported provider and explain
  what leaves the machine. Resolve additional credentials or a paid-service choice with
  the owner before enabling it; do not introduce an undisclosed background service.

## Required proof before shipping

1. Save a synthetic band-preparation/synchronization procedure, add unrelated teaching,
   save manually and through autosave, restart and load with a fresh assistant context.
   Both procedures must remain available without replaying any edit.
2. Omitted and empty fields cannot silently clear prior teaching. Duplicate updates do
   not multiply entries. Contradictions remain pending until explicitly resolved.
3. Explicit revision/deletion persists across compaction and restart. Restoring a prior
   version requires a deliberate action and reveals what changes.
4. Compaction reduces compressible notes while preserving protected content and pending
   tasks. Invalid summaries, concurrent changes, worker failures and disk-write failures
   preserve the valid original. Exercise Unicode and hostile size/count inputs.
5. Qualify the final signed app through the actual Update Assistant/autosave controls
   using generated data; preserve the owner's real assistant and project memory.

## Related approved release scope

Assistant Library naming and Window-menu access, OCR, PDF-parser compatibility,
on-demand page illustrations, increased library capacity and EPUB import are distinct
items included in the owner's complete next-build scope. Qualify each explicitly before
starting another release compilation; none is complete merely because the memory fix passes.
