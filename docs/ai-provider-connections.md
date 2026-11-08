# AI provider connections

The initial provider implementation was developed on `codex/ai-provider-connections` above
`41af6f81` (PLAYBACK.5, Console contract `filmcraft-console/9`) and integrated into main as
`94b40395`, with passive discovery repaired in `acc02d4a`. The latter passed a source-stable
4,281-test gate and matching signed CLI/native provider checks. Actual owner-driven Desktop
inspection, native marker/Undo and external voice use passed with scoped evidence. Existing-chat
reconnect did not: the client retains the old helper after configuration reload. Explicit rebind
passed an installed-client same-thread probe against a synthetic host, then the full source-stable
4,297-test/49-WASM gate in `a59e1c97`. Later matching native checks confirmed that background
inspection retained ChatGPT's foreground focus. The owner also verified same-chat reconnect with
`connection_rebind` in a new conversation using the updated helper. A legacy chat whose cached tool
catalog lacks that tool still could not recover; JSON handoff to a new chat is the owner's recorded
workaround. Automatic credential refresh or edit replay is not an acceptable workaround.
Live API edit/Undo acceptance remains open. See
[current evidence and limits](testing.md#current-alpha-acceptance-follow-up-2026-11-01).

## Connection ownership

The Console keeps Codex as its default and selects one provider at a time. Each connection gets a
fresh, revocable permit. Every managed-provider native request checks that permit again on the UI thread at execution.
Switch, disconnect, cancel and project rebinding revoke old access; queued old requests cannot edit
the new project. Credentials are not serialized into projects, chat or inspection results. The native
bridge preserves project/revision guards, direct commands, grouped Undo, analysis ownership, memory,
styles and selected-file snapshots. Provider histories have separate indexes and shared project memory.
Existing Codex transport, model selection, voice and its original saved-session namespace remain intact.

Codex connections request live web search at App Server launch and again in the per-thread
configuration, including resumed conversations. This enables Codex's hosted search tool; shell
commands, unrelated plugins/apps and other MCP servers keep their existing disabled policy.
Search availability remains subject to the installed Codex runtime and managed account policy.
Retrieved web content is reference material, not permission to edit projects or change settings.

When voice fails during a voice-originated Codex task, the recovery candidate sends the existing
turn interrupt once and waits for actual completion before allowing another voice session. A
30-second missing-acknowledgement limit reports that Codex reconnection is needed; it does not
claim the interrupted edit completed or replay it. Ordinary text-originated tasks are left alone.
The owner identified Cancel beside Send as the working manual recovery. Controller regressions
qualify the candidate's state transitions; matching packaged timeout/reconnect acceptance remains open.

Self-authored `assistant_update` checkpoints save current-project memory without invalidating the
active conversation's edit context. The assistant already knows what it wrote; forcing an idle-only
session refresh here stranded edits during active voice. Explicit assistant/style loads and actual
document changes retain the context guard. For a genuine pending refresh, choose Voice Off if active,
finish or cancel the current turn, wait for refresh, and inspect the current project before requesting
fresh edits. Read-only inspection may remain available while mutations are blocked. Do not replay an
old edit plan merely because inspection works. The owner-reported failure path is reproduced in a
synthetic regression; the repair still requires final source and packaged qualification.

## Desktop ChatGPT: external MCP conversation

There is no verified interface for driving an ordinary ChatGPT desktop composer from FilmCraft.
Desktop mode therefore attaches an external MCP client; it does not invent an embedded conversation,
a model-control API or an app-controlled voice toggle. Type or speak using the external client's own
controls. Owner-operated voice-to-tool inspection and marker/Undo passed on the current Mac;
updated-helper same-chat reconnect is qualified, with the legacy cached-tool limitation above.

Select Desktop ChatGPT and Connect. The host publishes a private, versioned descriptor at
`Console/Desktop/connection.json` beneath its explicit data directory. It contains the current
loopback port, control credential and ownership permit. A separate OS-backed lock permits one host
per profile. The descriptor is owner-only on Unix and removed on disconnect; credentials are never
placed in the setup path, project or chat.

`Console/Desktop/setup.json` contains only the matching CLI command and arguments:
`mcp --controller-console --provider-profile <absolute connection.json path>`. The client setup stays
the same when reconnect rotates credentials or changes the port. The CLI reads the bounded, strict,
private descriptor explicitly; it does not fall back to inherited credential environment variables.
Symlinks, unsupported versions, malformed credentials and non-Unicode configuration paths fail with
errors. Automatic client configuration import and automatic client restart are not implemented.

The documented desktop setup is Settings > MCP servers > Add server > STDIO; supply the matching
command and arguments, then start that client server after FilmCraft Connect. Merely generating
configuration or starting its helper is not Connected. The initialize request authenticates the
current permit. The client initialized notification enables tool calls but leaves inventory discovery
passive: Desktop can terminate a discovery-only helper without EOF. The first parsed native tool
request activates the attachment once; only that active helper renews the lease. Discovery never
claims Ready, renews an active lease, or revokes a different active helper on EOF. Incomplete
initialization never renews a lease. Client names are not attested ChatGPT identity: the app reports
an external MCP attachment. A two-second heartbeat renews a ten-second lease. Execution checks expiry
before UI polling; normal EOF detaches and unresponsive helpers expire. Provider/project changes
revoke access. Reconnect errors do not require an unrelated memory reload. Embedded Send/model/voice
controls are disabled; conversation, model selection, voice and history belong to the external client.

For an existing conversation whose helper retains revoked credentials, server reload alone may not
replace that helper. After the owner explicitly uses FilmCraft Connect, ask ChatGPT to reconnect to
FilmCraft using `connection_rebind` with `{"confirm":true}`. This operation reads only the original
private descriptor path and requires a fresh, previously unattempted permit. It returns the new
generation and inspected project identity, without replaying any earlier command. Inspect native
state before deciding whether a failed edit is still needed. Initial attachment is also one-shot:
a cancelled or uncertain attachment requires a fresh Connect rather than an automatic retry.

Rebind waits at most two seconds for existing native calls, which can otherwise run concurrently;
a busy connection is not cancelled. Profile loading has a two-second deadline and one worker slot
that stays occupied until the actual read exits, even after cancellation. Calls are pinned to their
starting generation; queued old-generation calls fail after a successful rebind. Old heartbeat
failures cannot revoke the replacement, and old cleanup uses only its old permit. Attempted permits
are quarantined in a bounded list (64 including the initial binding); exhausting it requires a helper
restart. A cancelled or failed candidate is not published or retried, and any unconfirmed remote
attachment expires without renewing. No credentials or caller-selected paths are accepted by the
rebind tool. Codex/API connections do not support this external-helper operation.


## OpenAI API: embedded composer and native tools

The API adapter uses the documented Responses API and the same matching native Console MCP helper.
It never reuses private Codex or ChatGPT authentication. Select **OpenAI API**, enter the key in
its masked **API key** field and specify the exact **API model ID**, then Connect. The key is kept
only for the app session, outside serialized UI state, project memory and diagnostics. Disconnect
before replacing it or using **Clear API key**; a still-closing worker must finish first. Clearing
also disables environment fallback for the rest of that app session. No model is silently substituted.
This in-app setup is under source and packaged acceptance verification.

Developer launch configuration remains available until in-app settings replace it:

- `OPENAI_API_KEY`: explicit API credential, never a Codex credential.
- `FILMCRAFT_OPENAI_MODEL`: default model ID.
- `FILMCRAFT_OPENAI_MODELS`: optional comma-separated catalog, at most 32 unique valid IDs,
  including the default. IDs are bounded to 256 ASCII characters, including colon-separated
  fine-tuned IDs; this does not add training functionality. In-app configuration uses its explicit
  model rather than inheriting this developer catalog.

Connect checks local setup/transport. API authentication, model access and actual modality support are
checked on a submitted request; UI model labels and capability inspection disclose that distinction.
The endpoint is fixed to the official HTTPS Responses endpoint, redirects are disabled and requests
are not automatically replayed. Network operations retain a separate 30-second timeout. Selected PNGs
and native PNG tool results are bounded, decoded for validation and delivered as typed image inputs,
including original transparency. Captures use a transient delivery buffer, dropped after the one
HTTP attempt resolves, including failure/cancellation; later requests require a fresh capture.
A separate five-turn generated-data `gpt-6-luna` smoke passed selected-image interpretation,
restart/resume, timeline capture and sanitized-history follow-up. This qualifies that isolated
corrected build and model test, not every model or the combined package’s live edit path.
Direct audio input, generated
speech and embedded API voice are not implemented. Native music/performance analysis remains part
of the shared command/tool catalog; it is not a promise of direct audio-model input.

API responses have bounded bytes/items/tool calls/rounds and validated identities. A failed or uncertain
native operation stops the turn; duplicate call IDs remain blocked across user turns and restarts.
The helper runs asynchronous duplex pipes under one write/read deadline with cancellation polling,
owned-child termination/reaping and bounded runtime shutdown. There is no blocking stdout-reader join.
Helper cancellation does not shorten the HTTP transport's separate timeout.

## Durable API resume

The host supplies its explicit data root; state lives under `Console/ApiSessions`. API display histories
use `api-sessions.json`, separate from Codex. Raw replayable Responses items, encrypted reasoning,
typed tool results, selected reference images and seen call IDs are stored together. FilmCraft-generated
frame/timeline captures are one-request inputs: their pixels are never checkpointed or replayed.
Retained history contains a discard notice and bounded numeric capture metadata instead. Selected
user input images and admitted `reference_image` snapshots retain their existing persistence.
Capture provenance comes from the matching native tool identity and admitted snapshot arguments,
not a result's claim to be a selected reference. Every journal save sanitizes legacy capture outputs;
opening an older valid session also scrubs them before resume or interrupted-turn rejection. A saved chat transcript alone is not
resumable state. No API keys, bridge tokens or permits are saved in this store.

Private atomic checkpoints and an OS-backed per-session lock permit one writer, including across data
file replacement; process exit releases the lock. Reads cap file bytes, JSON expansion/nodes/depth,
string sizes, history counts and graph structure. Unknown formats, traversal IDs, duplicate/orphaned
calls/results, malformed images and incomplete conversation boundaries are rejected before any request.

Before a turn can send HTTP or dispatch a native tool, it writes an in-flight checkpoint. Only a
completed turn publishes a clean checkpoint. Failed or interrupted turns cannot automatically resume
or replay; inspect native project/history/jobs and explicitly start a new conversation. Failure to save
a completed turn reports that edits may already have completed; it never reports rejection or retries.
Resume always supplies current memory/instructions, without replaying stored instructions as authority.

Saved scopes must match. Relative host data/project paths are resolved to absolute paths before
validation and persistence; non-Unicode paths return an error. First Save and Save As can migrate only through bounded aliases recorded by
the host while the same document identity remains open, including a disconnected session and archive
reload. An unrelated project cannot gain an alias simply by selecting an old conversation. Project
changes revoke native ownership; reconnect selects a matching API history or starts fresh. Old provider
conversation IDs are rejected before replacing ownership. Model access and cross-model opaque-history
compatibility remain API/server concerns and need live qualification; errors do not trigger replay.

## Qualification and remaining gates

Focused qualification includes 51 passing app, automation and real CLI process tests for API state,
transport cancellation, Desktop profile rotation/locking/cleanup, strict descriptor loading, explicit
profile precedence over invalid inherited credentials and real MCP initialization/EOF behavior. A
lightweight source-import harness passes 36 tests. A non-Unicode setup-serialization panic was first
reproduced synthetically, then fixed and regression tested. Two final API resume preflight and external-provider context-reconnect regression tests also pass
in the real UI crate.

The completed `full-ci-desktop-api2/` snapshot passed 4,241 native tests (50 skipped), strict workspace
Clippy, formatting, doctests, i18n, layers, assets and 49 WASM crates. Its before/after source hashes
match, but it predates the stable Desktop profile and latest compatibility fixes. It is not the final
gate for current source. A fresh full repository gate is required after these changes.

GUI/native acceptance, default Codex regression, actual Desktop attachment/voice and real API
authentication/model/image tests remain open. No paid request, GUI manipulation, demo replacement,
installation, commit, push or main integration occurred in this stage. The retained `full-ci-desktop-api3/` gate passed 4,255 native tests and 49 WASM crates, with matching
32-file source hashes, before the subsequent capture-discard correction. That correction requires
a new focused and source-stable full gate; it does not overwrite the earlier evidence.
Integration readiness remains false until the relevant live behavior and review are verified.

Exact snapshots/logs live in the ignored
`target/provider-connections-qualification-20261031/` evidence directory. The earlier `full-ci/` gate
passed 4,220 tests with 50 skipped and 49 WASM crates before Desktop/resume changes; it is not evidence
for these later changes. The Desktop-only app test build was deliberately interrupted (exit 130) to
include API resume in a combined gate, not recorded as passing. Historical cancellation proof and
superseded source remain in `pre-cancellation-fix/`; that API prototype defect is not a diagnosis of
the historical installed-demo hang.

## Attended acceptance sequence

Use only the matching development app/CLI, a separate explicit `--data-dir`, a unique control port
and a disposable project. Leave the installed demo and its lease untouched. Record build source
hashes, selected provider/model, native before/after revision and Undo results; do not record secrets.

1. Default Codex: inspect the disposable project, propose/apply a marker, Undo, disconnect and resume
   a saved conversation. Confirm native state and existing model/voice controls remain consistent.
2. Desktop: Connect, configure the stable STDIO helper in the external client, complete initialization
   and inspect project identity. Inventory-only helpers must remain passive; the first native inspection
   must activate the connection and survive past ten seconds. Apply one marker and Undo. Disconnect: old helper requests must fail
   without changing revision. Reconnect/restart the client with the unchanged setup, then repeat.
3. Desktop lifecycle: change project, terminate the owned helper and wait beyond the lease. Verify
   the badge and native access revoke. Check model/voice using the external client's own controls;
   a working text MCP session alone does not qualify voice-to-tool use or saved external history.
4. API (only with explicitly authorized credential/model and paid smoke test): inspect identity,
   execute one marker and Undo, inspect a selected generated PNG, then disconnect/restart/resume.
   Verify the exact model, restored conversation context and absence of replayed native edits.
5. Failure paths: cancel an in-flight operation, revoke ownership during a queued edit, try an
   unrelated project/session and attempt a concurrent resume. Confirm actionable errors, unchanged
   project for rejected edits, preserved Undo and no automatic uncertain-operation retry.

A failed step remains an open acceptance item. Repository checks and mocked model responses do not
substitute for these observations. No test requires using a real user project or exporting media.

## Official references checked 2026-10-31

- https://learn.chatgpt.com/docs/extend/mcp (desktop STDIO setup and Codex-host configuration)
- https://developers.openai.com/chatgpt
- https://developers.openai.com/api/docs/guides/function-calling
- https://developers.openai.com/api/docs/guides/conversation-state
- https://developers.openai.com/api/docs/guides/migrate-to-responses
- https://developers.openai.com/api/reference/resources/responses/methods/create (typed image tool output)
- https://developers.openai.com/plugins/build/mcp-events (hosted subscription route, not embedded composer)


## Codex speaking-voice selection

This follow-up and the persistent session lock are committed in `626aad8e` after a complete
source-stable gate (4,311 tests, 50 skipped). A matching full-feature package is building;
actual selected-voice acceptance remains pending. They are not in the `a59e1c97` reconnect package. The Console picker uses the current runtime's v3 voice catalog,
not a hard-coded list or another protocol version's default. Use Default omits the realtime
voice override entirely; inheritance of an account's personal preference is not asserted.

The choice is remembered as an application preference (`general.codexSpeakingVoice`, empty for
default), independently of projects and Undo. `console.voice.choice.set` accepts an available
voice identifier or `null` for default while voice is off and the turn is idle. An unavailable
explicit choice prevents microphone startup until the user chooses an available voice or default;
there is no silent substitution. Model selection remains independent. Desktop ChatGPT uses its
external voice controls, and embedded API voice remains unsupported.

Catalog discovery is asynchronous and bounded. Timeout or unsupported discovery leaves text and
default voice available. Reconnect clears stale choices without erasing the saved preference;
the worker validates again before preparing audio. A reply arriving during synchronous Resume
setup is retained once as bounded validated identifiers, then delivered to the chat loop. Large
unrelated metadata and private provider errors are discarded. Inspection exposes choice, catalog,
loading and error state without recording audio.

Six isolated helper tests passed for catalog bounds, default omission, explicit selection, response
ordering, error redaction and bounded reply floods. The complete preference/UI/transport passed
the full gate in `626aad8e`; the matching packaged voice check is still required.

### Codex conversation lock

The Codex Console has a **LOCK SESSION** control for its current conversation. Locking the current connected conversation is available during a voice call or active turn and does not stop voice, change microphone mute, or reconnect. Unlocking still requires an idle conversation with voice off.
An explicit `console.session.lock {"threadId":"<UUID>"}` can select a known
conversation while disconnected; `console.session.unlock {}` clears the lock.
While locked, Connect resumes only that ID, including when it has no local
FilmCraft display archive. New Conversation and other saved-session choices
are rejected until explicitly unlocked. A missing Codex thread is an error; it
does not create a replacement. Project memory still refreshes for the current
project and past approvals remain invalid after reconnect.

The versioned policy lives at `Console/codex-session-lock.json` beneath the
normal per-user FilmCraft data location (on macOS,
`~/Library/Application Support/FilmCraft`), independently of the executable,
portable bundle, `--data-dir`, or `FILMCRAFT_DATA_DIR`. Thus changing a demo build
does not clear it. Each connection attempt rereads the shared policy; malformed
or unreadable data blocks Codex connection rather than silently discarding the
lock. An explicit unlock is persisted. Desktop ChatGPT and API conversations
remain separate. The file contains only the conversation ID, never credentials
or project edits. FilmCraft display archives and Codex's own durable history
are retained; a lock is a routing preference, not a backup of Codex history.

### Microphone mode and voice interruption — working-tree candidate

The owner-requested interruption fix uses **Headphones — can interrupt** as the default Codex
microphone mode. Assistant output does not suppress microphone samples in this mode, allowing
near-end speech to reach the full-duplex voice service. **Speakers — echo reduction** retains
output-time suppression and its 300 ms tail; wait to reply in that mode. This is an explicit
routing choice, not acoustic echo cancellation. Desktop ChatGPT keeps its own external controls.

`console.voice.inputMode.set {"mode":"headphones"|"speakers"}` changes the next voice start's
application preference (`general.codexVoiceEchoReduction`, boolean). It requires voice off and
an idle turn, rejects unsupported providers and unknown values, and does not start devices or
edit the project. Mode configuration precedes Voice On; a failed configuration does not start
voice. Local mute, rapid-toggle protection and unmute backlog clearing remain independent.
Inspection exposes the selected mode; no audio samples are retained for this setting.

Seven exact draft-module tests qualified packet encoding and mute behavior, not device capture
or actual interruption. Full integrated qualification and an owner-attended headphones test in
the matching package are still required. OpenAI describes GPT-Live as full duplex in its
[session guide](https://developers.openai.com/api/docs/guides/live-conversations); that capability
alone does not prove FilmCraft's queued playback and actual Codex behavior after interruption.
