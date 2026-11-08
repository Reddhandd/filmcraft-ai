# Named Project Assistant autosave

Status: source candidate; focused qualification, full gate and matching packaged/native acceptance
are required before delivery. The existing demo is unchanged.

Select or save a named Project Assistant, then enable **Autosave** beside the selector. The option
belongs to this project's checkpoint and defaults off for existing projects. Loading another
assistant or choosing Current project memory turns it off. Manually updating the same assistant
keeps the option enabled and advances its saved baseline.

Changed reusable instructions are saved after 15 minutes, at existing memory milestones, and
before a clean project switch or close. Unchanged instructions do not rewrite the catalog. This
copies the current structured role, goal, preferences and workflow; it does not generate a new
summary, call a model, export the transcript, copy project-specific decisions or pending tasks,
or replay commands. Project memory continues to use its separate checkpoints.

The Console shows pending work, the last successful assistant save timestamp and actionable
failures. A single background writer compares the saved assistant with the last observed version
under the catalog lock, retains `assistants.previous.json`, then atomically publishes
`assistants.json`. A conflicting update from another project is reported rather than overwritten.
Review the saved assistant before re-enabling Autosave or manually updating it after a conflict.

Assistant switching and manual updates wait for an outstanding write. Close waits are bounded;
a timeout is not reported as success. Project memory is still checkpointed when a reusable
assistant save fails. The error remains visible so the user can resolve it or disable Autosave
before closing. All files remain in the existing preserved data profile.

Native controls use `console.assistant.autosave` with boolean `enabled`. The existing
`console.assistant.inspect` result includes `autosaveEnabled`, `assistantSavedAt`,
`autosavePending` and `autosaveError`. Parameters use the same validated command path as the UI;
no arbitrary path or executable history is accepted.

Qualification must cover old-checkpoint migration, opt-in reopen, interval timing, fresh-project
instruction loading, project-history isolation, concurrent writers, failed backups and storage
failures, late acknowledgements, bounded close, and native control/status behavior. A source test
is not evidence that a connected AI or the packaged window has passed acceptance.
