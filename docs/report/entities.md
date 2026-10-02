# Characters and Relationships Implementation Report

Work started from clean `main` at `71ef4b8` on branch `entities`.
This report is updated as evidence is collected. Unrun checks are not passes.

## Results

| Check | Result | Evidence |
| --- | --- | --- |
| C0 fresh database and preserved text-model selection | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c0-model.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c0-model.txt) |
| C1 tool contract comparison | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c1-contract-comparison.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c1-contract-comparison.json) |
| C1 permanent test count, before and after: 186 | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c1-verification.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c1-verification.txt) |
| C1 cargo test | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c1-cargo-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c1-cargo-test.txt) |
| C1 cargo clippy, zero warnings | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c1-cargo-clippy.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c1-cargo-clippy.txt) |
| C1 node tests | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c1-node-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c1-node-test.txt) |
| C1 TypeScript | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c1-tsc.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c1-tsc.txt) |
| C2 test count: 190, exactly four added | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c2-cargo-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c2-cargo-test.txt) |
| C2 unchanged C1 tool contract | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c2-contract-comparison.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c2-contract-comparison.json) |
| C2 redundant-state source scan | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c2-legacy-state-scan.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c2-legacy-state-scan.txt) |
| C2 cargo test | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c2-cargo-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c2-cargo-test.txt) |
| C2 cargo clippy, zero warnings | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c2-cargo-clippy.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c2-cargo-clippy.txt) |
| C2 node tests | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c2-node-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c2-node-test.txt) |
| C2 TypeScript | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c2-tsc.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c2-tsc.txt) |
| C3 cargo test: 219 | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c3-cargo-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c3-cargo-test.txt) |
| C3 cargo clippy, zero warnings | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c3-cargo-clippy.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c3-cargo-clippy.txt) |
| C3 node tests | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c3-node-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c3-node-test.txt) |
| C3 TypeScript | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c3-tsc.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c3-tsc.txt) |
| C3 retired-kind/tool and redundant-state source proofs | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c3-source-scans.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c3-source-scans.json) |
| C4 cargo test: 229 | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c4-cargo-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c4-cargo-test.txt) |
| C4 cargo clippy, zero warnings | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c4-cargo-clippy.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c4-cargo-clippy.txt) |
| C4 node tests | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c4-node-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c4-node-test.txt) |
| C4 TypeScript | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c4-tsc.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c4-tsc.txt) |
| C5 cargo test: 228 | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c5-cargo-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c5-cargo-test.txt) |
| C5 cargo clippy, zero warnings | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c5-cargo-clippy.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c5-cargo-clippy.txt) |
| C5 node tests | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c5-node-test.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c5-node-test.txt) |
| C5 TypeScript | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c5-tsc.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c5-tsc.txt) |
| C5 Scoped removal source proof | PASS | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c5-source-scans.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c5-source-scans.json) |

## Commits and Line Counts

Counts are derived from staged `git diff --numstat`, with test lines classified separately. Each commit's raw numstat, patch and classified totals are saved beside its check logs.

| Step / Commit | Production | Tests | Audit |
| --- | --- | --- | --- |
| C1 `e5e583a` | +929 / -870, net +59 | +411 / -389, net +22 | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c1-audit.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c1-audit.json) |
| C2 `ed707b6` | +99 / -87, net +12 | +202 / -70, net +132 | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c2-audit.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c2-audit.json) |
| C3 `c667184` | +1042 / -466, net +576 | +1140 / -75, net +1065 | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c3-audit.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c3-audit.json) |
| C4 `8a5947c` | +480 / -438, net +42 | +591 / -313, net +278 | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c4-audit.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c4-audit.json) |
| C5 `37f8ae9` | +57 / -122, net -65 | +56 / -120, net -64 | [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c5-audit.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c5-audit.json) |

C1 production net is within the estimate (+40 to +70). Test net is 22 rather than -5 to +10 because the relocated tests need per-module imports and test-module scaffolding. No permanent test was added or deleted. Gross counts include moved lines; raw numstat uses `--no-renames` for deterministic classification. The required initial `git mv` was performed; Git's final commit represents the multi-file split as deletion/additions because rename detection is content-based.

C2 exceeds the production estimate (-20 to 0 net) by 12 lines: explicit Live/Replay mode, story-scoped projection guards and joins, change-only SQL, and the final presence pass outweighed table/link deletions. The four required new tests exceed the estimated +30 to +45 lines because they exercise the actual erase command and verify full transaction rollback, untouched rows/transcript/turn/story timestamps, final-state name reuse and preserved creation time, plus single/bulk/wrong-story stat reads. No permanent C1 test was deleted.

C3 exceeds the +126 to +231 production and +150 to +200 test-line estimates. The implementation includes explicit raw/shown loaders, direction-conflict validation and reversed-mutual/tombstone priority, current-event parsing without old-payload fallbacks, strict typed/unknown-key validation in retained interim tools, two-phase registry decisions, exact-match rechecks, and deterministic blank-key failure before network access. Its 29 added tests use real turn/savepoint/erase paths, raw unique-index probes with rollback, semantic snapshots plus exact untouched timestamps, full registry/event atomicity checks and multiple invalid-argument cases. These fixtures and normally formatted Rust calls account for most of the test-line difference; the estimate substantially understated the executable coverage for its 12 multi-case groups. All requested cases are retained rather than compressed into opaque assertions.

C3 preflight caught one stale fixture that rejected partial tool settings (now valid by design), and two warnings (unused root re-exports and a needless test borrow). They were corrected before C3 was committed. The required post-commit checks passed on their first run. Saved preflight logs retain the failures and reruns. Independent read-only review found no actionable correctness issues: [C:\Users\User\Documents\GitHub\story-llm\docs\report\entities\c3-review.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/entities/c3-review.txt).

C4 net +42 production exceeds the estimated -95 to -15. Correct explicit-null serde patches require per-field deserializers and sparse serialization, shared trim/scalar-length normalization, static field metadata and apply helpers; command dispatch and subtype-aware snapshot reads add code even after deleting the three tool wrappers. Tests net +278 exceed +60 to +85 because all seven field combinations, both create/update bounds on all six short fields, Unicode boundaries, full logical snapshots, subtype fixtures and atomic new/existing-character stat preparation are executable, not merely schema checks. Ten net test functions were added; five retired tool tests were replaced by six save-character tests with equivalent coverage, not discarded.

C4 preflight retained evidence of a content-format mismatch, the follow-up richer-location expectation, the enlarged event enum warning, and a stale frontend `create_entity` settings fixture. Short-field changes now include a prior value when present; the `after` snapshot is boxed to reduce enum size without changing JSON; the frontend fixture uses `save_character`. A Windows evidence-runner encoding error was fixed to print UTF-8; its already-written Cargo failure log was preserved. All four required post-C4 checks passed on their first run.

C5 removes more than the estimated -10 to -5 production / +0 to +10 test net. Complete Scoped removal also eliminates now-unused `Inputs.history`/`config`, their actual `narrator/generation.rs` caller, configuration fixtures and test helpers, plus summary-line branches. Visibility cases were folded into existing tests; only the explicitly retired touched-entity query test function was removed (229 to 228). There is no dead-code allowance or compatibility select/mapping. Development and required post-commit checks both passed first time.

## Choices

- C0 found no `text_model_default` row. The effective defaults in `features/settings/model.rs` are `openrouter` and `x-ai/grok-4.7`; QA will use those rather than inventing a saved selection.
- Evidence helpers are kept under the ignored report directory, not committed as application code.
- C7 will give the QA story a title through the normal UI before the first narration. This avoids an unnecessary automatic title-generation call within the 15-call spending cap; narration/tool calls remain real calls.
- Relationship labels and directions are non-nullable database fields. Their schemas accept strings only, so explicit null is rejected; nullable descriptions and character facts use null to clear. Missing label/direction on an existing relationship leaves them unchanged, and a new relationship needs a nonempty label.
- A reversed mutual relationship request updates its stored orientation. Converting it to one-way in the reversed orientation is rejected, not silently flipped.
- Replay snapshot comparisons include all semantic relationship/stat fields; a relationship rebuild regenerates projection `updated_at`. Endpoint-only replay must leave even the relationship/stat timestamps unchanged, and a separate assertion checks that.
- Current entity events require their source and complete required before/after metadata. Old-payload fallbacks were removed; missing nullable link/character fields remain null as explicitly designed.
- The retired campaign difficulty dial, its starter attribute, the retired kinds, and old `dice_mode` / `attributes_enabled` stripping were removed together in C3 as the requested single cleanup item.
- The final tool catalog lists `save_character` before `save_relationship`, reflecting the endpoint dependency; C1's original catalog order was preserved during the behavior-neutral split.
- Short-field update content includes the previous value when one exists; clearing says "cleared", and appearance updates show the new look rather than duplicating a potentially long old appearance.
- Existing UI command name/anchor arguments remain while typed `fields` is authoritative when supplied. C6 makes the name optional to permit sparse saves and routes a relationship's name argument to its label; no database or settings compatibility conversion is added.
- UI relationship endpoint titles use `known_as` where set, so the Relationships tab does not bypass a character's collapsed true-name reveal. Backend/context relationship display names remain true names.
- The selected entity tab is a small per-story local UI preference, not another story Record/Show setting. Reveal defaults collapsed again when reopening the story.
- The actual context builder caller is `narrator/generation.rs`, not `stream.rs`; C5 removes its two obsolete input initializers and leaves stream/compaction's `raw_tail_boundary` use intact.

## Narrator Calls

Not yet run.

## Spending

QA text calls: 0. QA image calls: 0. QA caption calls: 0.
The final totals and cost will be captured read-only from `usage_records`.

## Merge

Not yet evaluated. `main` remains at `71ef4b8` until all required checks have acceptable, saved evidence.
