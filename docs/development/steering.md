# Mid-turn steering vs. the durable queue

A message the user types while a turn is running can take one of two paths.
Both are fully implemented and unit-tested. Only one of them is reachable from
Cockpit. This document records what exists, what the two behaviors actually do
differently, one concrete UX proposal, the transcript record steering is
missing, and the questions a human must answer before any of it ships.

This is a decision document. Nothing here has been implemented.

## What exists today

### Path A — queue (what Enter does today)

`SessionView.submit()` (`apps/cockpit/src/views/SessionView.tsx:277`) →
`enqueueQueueMessage` (`apps/cockpit/src/views/SessionView.tsx:292`) →
the `enqueue_session_message` verb
(`crates/core/src/api/sessions.rs:434`) →
`ControlPlane::enqueue_session_prompt`
(`crates/core/src/control/lifecycle.rs:694`) → a durable row in the
session-prompts table → rendered in the strip above the composer
(`apps/cockpit/src/components/session/QueuedMessages.tsx:19`, invisible when
the queue is empty) → delivered as a whole new turn by
`ControlPlane::deliver_next_queued_session_prompt`
(`crates/core/src/control/lifecycle.rs:710`) once the session goes idle.

### Path B — steer (implemented, unreachable)

`useStore.send` (`apps/cockpit/src/store.ts:613`) → `commands.steerSession`
(`apps/cockpit/src/bindings.ts:80`, generated) → the `steer` verb
(`crates/core/src/api/sessions.rs:251`) → `ControlPlane::steer_session`
(`crates/core/src/control/lifecycle.rs:960`) → `NativeSession::steer`
(`crates/core/src/harness/native/mod.rs:848`) → `SteerBuffer::push`
(`crates/core/src/harness/native/steer.rs:26`) → drained by the running
`drive()` loop at `crates/core/src/harness/native/runner.rs:1644` and folded
into the current turn's next provider request.

There are three drain sites in `drive()` — after a tool-result batch
(`runner.rs:1644`), on a tool-less round (`runner.rs:1709`), and after the loop
(`runner.rs:1788`) — so a steer is never silently dropped by the parent drive.

### The defect

`SessionView.tsx:277` returns early into the queue path for every
send-while-running, so the store's steer branch at
`apps/cockpit/src/store.ts:612-613` is unreachable from the UI even though it
is unit-tested (`apps/cockpit/src/store.test.ts:1310`). The engine, the RPC
verb (`crates/core/src/api/sessions.rs:251`), the Tauri command
(`apps/cockpit/src-tauri/src/commands.rs:364`), the generated binding and the
store branch all work; nothing in the product calls them.

The cost is not zero. `steer_channel_note()`
(`crates/core/src/harness/native/context.rs:34`) spends tokens in *every*
system prompt teaching a contract no user can exercise.

### The marker contract

`STEER_MARKER_OPEN` and `STEER_MARKER_CLOSE`
(`crates/core/src/harness/native/steer.rs:18-19`) are the single source of
truth for the wrapper that gives steered text user authority. The strings
`steer_channel_note()` (`crates/core/src/harness/native/context.rs:34`) puts in
the system prompt are the same constants, interpolated — not a copy — and must
stay byte-identical. `crates/core/src/harness/native/steer.rs` must remain the
only writer of the pair: the system prompt tells the model that text inside it,
and only text inside it, comes from the user, so any other content path able to
emit the markers would let tool output impersonate the user.
`harness::native::context::tests::assembled_system_teaches_the_verbatim_steer_marker`
pins this.

### Prerequisite: sub-agent isolation has landed

The known blocker is fixed. `deps_for_subagent` now hands a child drive a fresh
`SteerBuffer` (`crates/core/src/harness/native/runner.rs:1991`), and all three
drain sites are gated on `DisplayMode::owns_steer()`
(`crates/core/src/harness/native/runner.rs:1044`), so a steer that lands while
a `task`/`delegate_agent` sub-agent is in flight is consumed by the parent turn
instead of vanishing into the child's ephemeral, unpersisted history. Covered
by `subagent_deps_get_a_fresh_steer_buffer` and
`steer_during_a_subagent_lands_in_the_parent_not_the_child`
(`crates/core/src/harness/native/runner.rs`).

One gap to know about: the fresh buffer alone makes a child's drain harmless,
so removing the `owns_steer()` gate would not fail any existing test. The gate
is defence in depth with no test of its own.

## What the two behaviors do differently

| Dimension | Queue (today) | Steer (unreachable) |
|---|---|---|
| When it reaches the model | after the current turn finishes, as a new turn | inside the current turn, appended to the next tool-result batch |
| Effect on the running turn | none | the model can change course mid-task |
| Durability | durable row, survives daemon restart, re-delivered on boot | in-memory `Arc<Mutex<Vec<String>>>`, lost on restart |
| Visible before delivery | yes — `QueuedMessages` strip above the composer, removable with an X | no — nothing is shown anywhere |
| Transcript record | yes — `run_turn` writes a `user`/`text` row (`runner.rs:585`) | **none** — text only enters the provider ledger |
| Cancellable by the user | yes, `remove_session_message` | no |
| Carries attachments / mentions / context refs | yes (`ChatRequestOptions`) | **no — text only** (`store.ts:613` sends `turn.text`) |
| Read-only-session guard server-side | yes (`sessions.rs:443`) | no — Cockpit-side gate only |
| Ordering with other messages | FIFO, one head per successful turn | all buffered messages are merged into one block, in push order |
| Behavior when the session is not actually running | n/a | falls back to a new turn and returns `false` (`lifecycle.rs:972`) |

In plain language:

Queue means *finish what you are doing, then do this next.* The agent completes
the task it is on, and the message becomes the opening of the next turn.

Steer means *stop, read this now, and take it into account for the rest of what
you are doing.* The agent sees the message between tool calls and can abandon
or redirect the work in flight.

The consequence that decides most of the design: **steering silently drops
attachments, structured `@agent` mentions, and context references.** Only
`turn.text` crosses `steerSession` (`apps/cockpit/src/store.ts:613`); the
queue path carries a full `ChatRequestOptions`. Any UX that offers steering
must therefore either refuse to steer when the composer holds attachments,
mentions or context refs, or extend the RPC. See Q4.

## Proposed UX

A proposal, not a decision — the decision is Q1.

### Send-mode control

Mount a `Segmented` control in the composer action row of `SessionView.tsx`
(the row at `apps/cockpit/src/views/SessionView.tsx:541-573` that today holds
Attach, `SessionCostPanel`, Voice and the Send/Stop button), using the existing
`@ryuzi/ui` primitive at `packages/ui/src/components/ui/segmented.tsx:15` with
`size="sm"` and options
`[{ id: "steer", label: "Steer" }, { id: "queue", label: "Queue" }]`.

It is **always mounted** and `disabled={!running}` — never conditionally
rendered. `AGENTS.md` requires stable dimensions for toolbars and panels, and
the action row must not change height or width when a turn starts or ends. The
composer already swaps Send↔Stop in place at identical size
(`apps/cockpit/src/views/SessionView.tsx:564`); the mode control follows the
same rule.

No new card and no nested card: the control lives inside the existing
`acrylic-card` composer surface.

### Placeholder

The placeholder at `apps/cockpit/src/views/SessionView.tsx:523` becomes
mode-aware while running — `"Enter to steer this turn"` /
`"Enter to queue for the next turn"` — keeping the non-running
(`"Ask for follow-up changes"`) and read-only (`composeReadOnlyReason`) strings
exactly as they are today.

### Persistence

The mode is remembered per session in the `useNav` store and persisted to
`localStorage` under a new key, following the drafts pattern at
`apps/cockpit/src/store-nav.ts:113`: a pure `read*` / `upsert*` helper pair with
its own unit tests, so a corrupt `localStorage` value can never take the
composer down.

### Forced fallback

When the composer holds attachments, structured mentions, or context
references, the "Steer" segment is disabled and the send uses Queue regardless
of the remembered mode, with the reason surfaced in the control's `title`.
Silently dropping a user's attachment is worse than ignoring their mode
preference.

### Failure legibility

`steerSession` resolves a `boolean`. `false` means there was no live handle and
the engine already fell back to a fresh turn
(`crates/core/src/control/lifecycle.rs:972`). The UI must say so with a
`sonner` toast — "No live turn — sent as a new message" — because otherwise the
user cannot tell which of the two behaviors they got. Today
`apps/cockpit/src/store.ts:612-619` discards that boolean entirely — `send`
returns only `res.status === "ok"`.

### Rejected alternatives

- **A `MenuPanel` dropdown.** Rejected: `MenuPanel` is for action menus and
  composer-anchored autocompletes, not value selection, and a send mode is a
  value. `AGENTS.md` puts value selection on `Combobox` or `Segmented`.
- **A modifier key (Shift+Enter / Cmd+Enter) as the only affordance.**
  Rejected: invisible — a user who does not already know steering exists never
  discovers it — and Shift+Enter already inserts a newline
  (`apps/cockpit/src/views/SessionView.tsx:504`).

## Transcript record

Steering **must** write a transcript row. This is a decision, not an option.
Without one the user sends an instruction and sees no evidence anywhere that it
existed — not that they said it, not that the agent received it. The queue path
writes a row (`crates/core/src/harness/native/runner.rs:585`) and the asymmetry
is not defensible for a user-facing action.

**Row shape.** `role = "user"`, `block_type = "text"`, payload
`{"text": <the message the user typed>}` — identical in shape to what
`user_row_payload` (`crates/core/src/harness/native/runner.rs:790`) produces
for an ordinary turn, so the transcript renderer and the composer's input
history need no change.

**Where it is written.** In `ControlPlane::steer_session`
(`crates/core/src/control/lifecycle.rs:960`), on the **`Some(handle)` branch
only**, following the `emit_status` pattern at
`crates/core/src/control/lifecycle.rs:980-997`: `store.insert_message(...)`
plus a `CoreEvent::Message { .., run_id: None, .. }` broadcast so live
subscribers render it immediately. Session-level `insert_message`, not
`insert_run_message`, because the control plane does not know the running
`run_id`.

**Hazard 1 — branch placement is load-bearing.** On the `None` branch
`steer_session` falls back to `continue_session`
(`crates/core/src/control/lifecycle.rs:972`), and that path's `run_turn`
already writes the user row. Calling `insert_message` unconditionally in
`steer_session` would produce a duplicated user message in the transcript.

**Hazard 2 — the trait default is a silent drop.** `HarnessSession::steer` has
a no-op default body (`crates/core/src/harness/mod.rs:222`). A future
non-native harness would accept the transcript row and drop the message, which
is worse than today's silence. The mitigation is to change the trait method to
return `bool` (default `false`) and only write the row when the harness
accepted — routed to Q5, not decided here. At this commit `NativeSession` is
the only non-test implementor in the workspace.

**No schema migration is needed.** This reuses the existing messages table, so
`LATEST_VERSION` and `crates/core/tests/fixtures/baseline_schema.sql` are
untouched.

## Open questions

These need a human answer. A recommendation below is not an answer.

- **Q1 — What should Enter do while a turn is running by default?**
  Options: (a) keep Queue as the default and make Steer an explicit choice;
  (b) make Steer the default and Queue the explicit choice; (c) no default —
  make the mode a required, remembered per-session choice.
  Recommendation: (a) for the first release; it preserves today's behavior for
  every existing user, and steering is the more surprising of the two.
  Blocks: the whole implementation.
  A fourth answer is legitimate: **remove steering instead.** If that is the
  call, the deletion is `crates/core/src/harness/native/steer.rs`, the three
  drain sites and the `owns_steer()` gate in
  `crates/core/src/harness/native/runner.rs`, `steer_channel_note()` in
  `crates/core/src/harness/native/context.rs` (which removes tokens from every
  system prompt), `steer_session` in `crates/core/src/control/lifecycle.rs`,
  the `steer` verb in `crates/core/src/api/sessions.rs`, `steer_session` in
  `apps/cockpit/src-tauri/src/commands.rs`, the branch in
  `apps/cockpit/src/store.ts`, and a `cargo gen-bindings` run.

- **Q2 — Should the mode be remembered per session, per project, or globally?**
  Recommendation: per session, following the drafts map
  (`apps/cockpit/src/store-nav.ts:113`).
  Blocks: the store shape.

- **Q3 — Should a steer be visually distinguished in the transcript?**
  Options: (a) render it as an ordinary user row, zero renderer change;
  (b) add a payload flag (e.g. `"steer": true`) and a subtle marker in the
  transcript renderer.
  Recommendation: (a) first, (b) as a follow-up if user testing shows
  confusion — a plain user row appearing mid-stream may read as "a new turn
  started".
  Blocks: whether the transcript components change at all.

- **Q4 — What happens to attachments, `@agent` mentions and context references
  on a steer?**
  Options: (a) forbid steering while the composer holds any of them, as in
  Proposed UX above; (b) extend the `steer` RPC and `SteerBuffer` to carry them.
  Recommendation: (a) — (b) is a much larger change that touches the
  provider-ledger append path.
  Blocks: the composer gating logic.

- **Q5 — Should `HarnessSession::steer` return an acceptance boolean?**
  Recommendation: yes, but only if a second harness implementation is on the
  roadmap; at this commit `NativeSession` is the only production implementor
  and the no-op default (`crates/core/src/harness/mod.rs:222`) affects test
  fakes only.
  Blocks: whether the transcript row is written optimistically.

- **Q6 — Does the `steer` RPC need the server-side read-only guard that
  `enqueue_session_message` has (`crates/core/src/api/sessions.rs:443`)?**
  Recommendation: yes — a client-side-only gate is not a guard, and the queue
  verb sets the precedent. Today only Cockpit's `composeReadOnly` keeps
  steering away from legacy / deleted-owner sessions, asserted client-side at
  `apps/cockpit/e2e/app.e2e.ts:883`.
  Blocks: nothing in the UI, but it should land in the same change.

### Notes for whoever picks this up

A reviewer should scrutinize two rows of the table above. The "carries
attachments / mentions / context refs" row is the single fact most likely to
change the recommended UX. The "transcript record" row is the reason this is a
design document and not a two-line fix.

Deliberately deferred: extending the `steer` RPC to carry attachments and
mentions (Q4b); a distinct transcript rendering for steered messages (Q3b);
making `HarnessSession::steer` fallible (Q5); and the server-side read-only
guard on the `steer` verb (Q6). File each as a follow-up once Q1–Q6 are
answered.

For completeness, one adjacent channel is *not* a third path: an
`askuserquestion` or `exitplanmode` response (see `approvals.md`) also carries
user text into a running turn, but the model solicits it and it arrives as a
tool result, not as an unsolicited user message. Queue and steer remain the
only two paths for a message the user sends on their own initiative.
