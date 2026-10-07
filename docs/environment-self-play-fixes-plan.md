# Sequential Card Market: interaction fixes and balanced opening priority

Status: implementation plan only. No code, saved definition, execution policy or run has changed.

## Evidence and scope

The completed 30-hand self-play matches used definition `sequential-card-market-stateless`, version `5d32a9420d58443c21af6f35fb85affcd62d1e9e7e24f61c42215bddcaf803a5`:

- Luna: `6e827ac5-306c-4c95-ab27-110951f25b77`, seat 1 -261, seat 2 +261.
- DeepSeek V4 Flash: `319799a2-c910-4876-bc5b-acb7c5b219c6`, seat 1 +78, seat 2 -78.

The audit replayed all 770 receipts and matched both full checkpoints. It verified 385 wake states and 768 result states. Card ownership, prices, settlement and accounting passed. Role-preserving seat-ID changes produced equivalent results. The worst Luna trades had correct supplied state, so these fixes are not a claim that the environment caused all losses.

The audit did find obsolete turn prompts, an accepted action based on an older view, and gaps in stateless chat/history. Opening priority was also intentionally unequal: seat 1 opened 60 rounds and seat 2 opened 30. The user now requests alternating priority between hands.

Evidence: `~/.prime-env-harness/session-artifacts/01a10c7c-c698-741b-9f67-15f653284209/self-play-30-hand-proposal/seat-asymmetry-investigation/` contains the summary, three audits and reproducible engine replay.

## 1. Alternate opening priority between hands

Change the environment definition, not the generic harness. Use roster position, never model names or seat-ID strings.

For a trading hand, define:

```text
hand_index = number of hands already settled       # zero-based while trading
round_index = current trading round                # zero-based
opener_index = (hand_index + round_index) % 2
opener = player_order[opener_index]
```

Apply the same calculation when dealing a hand and when advancing to its next round. Do not use the newly incremented settlement count to choose a finished hand's opener.

For three rounds:

| Hand | Round 1 | Round 2 | Round 3 |
| --- | --- | --- | --- |
| 1 | Seat 1 | Seat 2 | Seat 1 |
| 2 | Seat 2 | Seat 1 | Seat 2 |
| 3 | Seat 1 | Seat 2 | Seat 1 |
| 4 | Seat 2 | Seat 1 | Seat 2 |

Each pair of hands gives both seats three openings. A 30-hand, three-round match gives each seat 45. For any odd round count, the extra opening alternates between hands. Even round counts remain balanced within each hand. An odd total number of odd-round hands necessarily leaves one extra opening overall.

Keep the existing `next_hand` controller, the first roster seat. This is an explicit scope choice: the user requested symmetric market opening, not a change to settlement administration. After `next_hand`, the controller might not be the opener. Its returned state must say to wait, and the environment must wake the actual opener. The fixes below prevent the controller's old task from starting another action. There remains an administrative workload difference of one `next_hand` call per hand; do not describe the whole seat experience as identical.

Preserve these rules:

- Executed quote owners reopen after a trade that does not end a round.
- At a round boundary, the calculated opener takes priority over the executed quote owner.
- Card distribution, settlement formula, one-contract fills, cash/position signs and reset rules remain unchanged.

Expose the current hand's opening priority in the authoritative view. Derive it from committed state; do not add another stored counter. Update `run_rules`, manifest instructions, observation field descriptions and report assumptions. Remove wording that seat 1 always opens odd-numbered rounds.

Checks: exact opener/active-seat/notification sequences for two consecutive hands with 1, 3 and 4 rounds; 30-hand opening counts; non-round-ending provider reopening; settlement and restart/restore parity; renamed seats; `next_hand` returning an inactive controller on alternating hands. Tests should assert behavior and state, not instruction strings alone.

## 2. Prevent accepted actions against a view the model has not seen

Luna hand 10 round 1 demonstrated the problem. Its last supplied state showed its own 43/45 market and no legal actions. The opponent improved to 43/44. Luna's later buy was legal at dispatch and executed against the unseen newer quote.

Bind every model-generated environment mutation to the committed state used for that model request. Carry an opaque view/version identifier through the host/seat protocol, outside model-authored arguments. The model should not calculate or choose a revision.

Implementation requirements:

1. Produce a filtered view and its version atomically from one committed state.
2. Freeze that version for the model request producing a tool call. A later background refresh must not silently replace it while the request is in flight.
3. At mutation dispatch, compare the expected version with current committed state under the same lock that protects the mutation. Reject a mismatch before invoking environment code.
4. Return a structured stale-view result plus a fresh filtered view. Let the model reconsider. Never replay the original action automatically, including a buy that becomes legal later.
5. After an accepted action or an explicit refresh, use the returned version for the next model request. Multiple precomputed tool calls cannot silently inherit a post-action version they were not generated from.
6. Repeating an already committed operation ID must return its original receipt before stale-version rejection. Retain existing recovery behavior for uncertain calls.

Use existing checkpoint operation sequencing as a conservative engine epoch rather than adding a separate counter. An engine `invoke` or `handle_input` receipt may invalidate older views even if the call fails, does not change its snapshot, or declares a read-only action. Accept that conservative rejection as the initial design; return a fresh view for reconsideration. Ordinary `Observe` and `AgentView` create no engine receipts, so those reads do not invalidate a view. Turn accounting, direct messages and participant Python namespace bookkeeping must not advance this engine epoch. Do not use `progress.revision`: it describes attention/progress and need not cover every mutation. Keep the versioned view and guarded mutation atomic inside `ActiveInstance.runtime`'s mutex. A daemon gate followed by a separate checkpoint read and invoke is not sufficient.

Own this at the generic execution boundary: shared wire vocabulary in `pa-types`, atomic view/check/mutation behavior in `pa-environments`, authenticated transport/admission in `pa-daemon`, and model-request/tool-call binding in `pa-core`. Final API shape should follow existing crate boundaries. No card-market formulas belong in these crates.

Bind each tool-call ID to its generating model request, not to a mutable seat-wide latest view. A tool reply or queued wake must not rebind the remaining calls from that response. Cover participant Python when enabled: `env.invoke` must respect the view used to generate the running cell. A returned invoke view must not authorize a second precomputed mutation from that same cell automatically. Require a fresh model request, or define and test an explicit Python observe-and-reconsider contract separately. Human forms must bind submissions to the displayed view. If that view is stale, reject the submission, render the refreshed state, and require the user to reconsider before submitting again. Silently refreshing and rebinding old arguments at dispatch is unsafe. Apply the same displayed/request-view guard to every mutation entry path. Builder read APIs do not act. Keep policy grants unchanged.

Regression tests: reproduce H10's inactive-to-active race; peer quote changes while a model request runs; illegal-to-legal and legal-to-illegal transitions; hand boundaries; consecutive same-seat actions; multiple tool calls; read-only calls; committed operation-ID retry after later mutations; restart and uncertain-operation recovery. Every stale rejection must leave engine state and external effects untouched. Use observable readiness, not sleeps.

## 3. Retire obsolete turn tasks without losing messages

A queued wake can contain “Your trading turn” or “Start the next hand” after that opportunity was consumed within an earlier turn. Refreshing only its state leaves contradictory instructions.

Represent actionable attention separately from ordinary notices and direct messages. Associate it with the committed version/attention state that created it. Do not classify turn prompts by matching English strings or by parsing a game-specific `active_player` field in the harness.

At turn admission, refresh state and actionable attention together. If a pure action wake is obsolete and the seat is no longer awaited, settle/ack it durably without starting a model request. If that seat is currently awaited for a newer opportunity, deliver the current task instead. Preserve meaningful private messages and non-action notices even if the recipient cannot act; those may justify a read-only model turn.

The environment decides tasks and recipients; the host revalidates their applicability. Retain targeted retry semantics for a genuinely awaited seat that consumed a wake without acting. Suppressing old attention must not swallow the only wake for the current legal player.

Primary seams: `environment_host/dispatch.rs`, `worker/environment_wake.rs`, `worker/turn.rs` and existing inbox/recovery tests. Add narrowly scoped shared metadata only where needed. Preserve durable admission before ACK and restore/replay behavior.

Tests: same-turn `next_hand` then market opening consumes an older wake; a round boundary leaves the same seat active; peer progress occurs while a wake waits; obsolete task plus private message; valid retry; crash between persistence/admission/ACK. Compare actual model request counts and delivered state/task pairs, not just queue length.

## 4. Make public chat visible to stateless peers

Treat `say` as public to both players as well as the spectator table. Keep `wake=False`: chat must not start an unsolicited turn or change trading priority.

In the card environment, retain a bounded public chat/event history with sender, hand/round and sequence, then include it in `agent_view`. All players get the same public record. Do not put private notices or assistant reasoning in that field. The host already refreshes action results, so the next normal wake/result can include messages received since the previous view.

Prefer one retained event source for trading and chat with an explicit event kind, rather than separate copies requiring synchronization. Preserve useful ordering and omission counts. Ensure chat-only state changes invalidate old views under the new version contract, without counting as game progress or triggering another seat wake.

Tests: one seat's `say` appears in both subsequent views; no unsolicited peer wake; preserved order through snapshot/restore; bounded size and explicit omitted count; no private content leakage. Report public chat separately from game actions and fills.

## 5. Provide the history the instructions promise

Recent `past_hands` currently contain cards, board, settlement and P&L, but not the trading behavior that produced them. Add a compact public trade/quote history for recent completed hands, or remove the strategy-learning claim if that history is not desired.

The proposed implementation retains a bounded chronological public action summary: round, actor, action, posted bid/ask or execution side/price, plus settlement. Do not export private model reasoning or undisclosed opponent cards. Current-hand views reveal only public events and the recipient's own cards; settled-hand history can include both disclosed hands.

Keep the existing 10-hand and 30-event limits unless size checks show they need adjustment. Bound serialized view size as well as record count, since chat can be much larger than a trade. State exactly what was omitted rather than silently dropping records. Avoid repeating generic rules in every view.

Use one-based round numbers consistently in participant-facing responses, including `say`; internal zero-based counters may remain. Tests should check action/view/say agreement, history bounds, restore and secrecy. Reports must distinguish actual delivered views from reconstructed committed engine states.

## Delivery order and validation

1. Implement and verify atomic view/action binding. Address all mutation paths and idempotency first.
2. Fix stale task admission using explicit metadata and durable inbox semantics.
3. Update the card definition for alternating openers and bounded public chat/trading history. Update rules and participant-facing round fields together.
4. Run focused regression tests and scoped compilation during iteration. Run `CARGO_BUILD_JOBS=2 make check` once for final Rust validation. Run `make deny` only if dependencies change; prefer no new dependencies. Preserve unrelated checkout changes.
5. Validate the new draft under the already selected local policy. Do not widen Python, network, mounts, secrets or integrations. Save it as a new immutable definition version. Existing instances remain pinned to their old version and are not migrated or resumed.
6. Rebuild and restart the daemon before testing changed host/seat code. Show the new draft, actual run settings, opening schedule and validation evidence to the user. Obtain separate authorization before any token-consuming run.

Harness changes stay within the existing environment extension. Document crate ownership and any new public/wire surface. Add adoption telemetry for new user-visible behavior without prompt, card, chat or file content; version-rejection properties can be primitive reason/count values. The fork's environment features are exempt from upstream TS parity, but ordinary builder behavior must remain unchanged.

After authorization, run a short fake-seat integration check first. Then use paired fixed deals with balanced role order for model experiments. Do not silently introduce deterministic dealing into ordinary games. Preserve models, reasoning levels, policy and non-opening trading rules when comparing runs. Compare stale-action rejections, inactive wake counts, view sizes and P&L. A different winner is not a correctness test.

## Acceptance criteria

- No action executes against a different committed state from the one supplied for its generating model request.
- Consumed turn tasks do not start redundant model turns or contradict refreshed attention.
- Public chat reaches stateless peers on their next normal view, without changing whose turn it is.
- Recent trading history and instructions agree; observations stay bounded and private cards stay private until settlement.
- Hand 1 openers are 1/2/1; hand 2 openers are 2/1/2; 30 three-round hands give 45 openings per seat.
- Existing accounting, legality, persistence, idempotency and recovery verifiers still pass.

No implementation constants changed in this planning task. The only proposed game-rule change is hand-alternating opening priority. Keeping seat 1 as `next_hand` controller and making public chat player-visible are explicit assumptions in this plan.
