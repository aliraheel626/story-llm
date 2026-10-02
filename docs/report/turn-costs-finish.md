# Turn Costs: Final QA And Integration

**Result:** All seven F4 checks passed. Local `main` fast-forwarded from `cab909d` to `edf77f8` and is clean; `origin/main` remains at `cab909d`. Nothing was pushed. The normal-ID Tauri app was stopped afterward, and the QA story `565120b9-4af1-4b4c-a81b-fd74a02e0f1e` ("The Room Holds Its Breath") remains for inspection. [QA gate and spend](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-summary.json), [12-commit main log](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f5-main.txt).

## F1: Do-Turn Probe

The script [do_variants.py](file:///C:/Users/User/Documents/GitHub/story-llm/docs/probes/do_variants.py) reused the logged `<see>the pebble</see>` request's system prompt, history and `<additional_instructions>`, replacing only the final turn with `<do>Pick up the pebble and turn it toward the window light.</do>`. It offered only `illustrate_scene` under `tool_choice: auto`. The logged request was available, so the hand-written fallback was not used. The key was read only into memory and was never printed or saved. [All ten classifications and individual costs](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f1-do-variants.jsonl).

| Do variant | Tool called | Tool text leaked | Substantial story prose | Cost |
| --- | ---: | ---: | ---: | ---: |
| Current warning | 0/5 | 0/5 | 5/5 | $0.020462 |
| Warning removed in both places | 0/5 | 0/5 | 5/5 | $0.022278 |

**Decision:** Apply the full rewording. Reworded did not produce more leaks than current (both 0/5). No Do run leaked tool text, so the out-of-scope narration guard is **not** a follow-up required by these results. The sample cannot prove a leak will never occur.

## F2: Prompt And See Confirmation

Commit `34a1ff3` removes the warning sentence from the `<see>` rule and replaces the tool description's warning with "Call it at most once per turn." `Your replies are only narration or real tool calls.`, See's required tool choice, and the See text-stream filter were unchanged. `rg -n "never write the call out" src-tauri/src` returned no matches. The six new forced-tool probes all called the tool: [individual See results](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f2-see-confirm.jsonl), [earlier comparison results](file:///C:/Users/User/Documents/GitHub/story-llm/docs/probes/see_variants_results.jsonl).

| See variant (tool required) | Clean | Tool-call text | Story prose in text | No tool |
| --- | ---: | ---: | ---: | ---: |
| Earlier warning, Claude's probes | 0/5 | 5/5 | 0/5 | 0/5 |
| Earlier warning removed in both places, Claude's probes | 6/9 | 1/9 | 2/9 | 0/9 |
| **Committed full rewording, new probes** | **3/6** | **1/6** | **2/6** | **0/6** |

The new tool-text rate (1/6) meets the at-most-2/6 confirmation threshold. Wording alone is not reliable: `d71eb92` intentionally suppresses *all* See text in the stream, and this was checked in the app.

## F3: Early Narration Error

Commit `edf77f8` includes `story_id` in backend and frontend `narration-error` payloads and replays an error when it arrives before submit/retry IPC installs the stream. It discards stored early failures if IPC rejects. Both new regression tests first failed with a stranded `streaming` value on the old implementation and passed after the fix: **early See failure does not leave the composer streaming** and **early Retry failure restores the original reply**. The tests assert `streaming === null`, `requestPending === false`, the error message, and retained original Retry entry. [Passing targeted test output](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f3-targeted-tests.txt). The failure race cannot be induced on demand in the running app; that part is covered by unit tests, not presented as in-app QA.

After **each** of F2 and F3, `cargo test --manifest-path src-tauri/Cargo.toml` passed 177 Rust tests, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` finished without warnings, `node --test src/features/story/store.test.mjs src/features/transcript/replacement.test.mjs src/features/usage/store.test.mjs` passed 45 (F2) then 47 (F3) tests, and `npx tsc --noEmit` passed. The frontend Vite test processes briefly reported a competing WebSocket port 24678 but exited successfully with all assertions passing.

## F4: Running-App QA

Clicked **+ New story**, which reused an existing unplayed blank "New story" as the application's normal creation policy. All six narrator-tool toggles, including **Illustrate scenes**, were on. [Initial story identifiers](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-setup-stories.json), [initial all-tools-on UI](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-setup-ui.json). All interaction was through tauri-pilot clicks and typing. Captures evaluated DOM read-only, and SQLite used `mode=ro`; no app state, DOM or row was altered by a probe.

| Check | Result | Full-Path Evidence |
| --- | --- | --- |
| 01. Do turn with tools on | **Pass.** `Look around the room.` saved no tool-call text. Its `$0.0244` text cost is immediately after narration and before the narrator-initiated image; that turn's text usage is $0.024392. No dollar value occurs in the Edit/Retry/Erase action row. The automatic image counts as image one. | [asserted result](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-01-do.json), [DOM order](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-01-do-ui.json), [transcript and usage](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-01-do-db.json) |
| 02. See stream and timer | **Pass.** Five live placeholder snapshots span 1,843 ms and show 2.4 s increasing to 4.3 s. The streaming reply was empty throughout, including no `{`, `description` or `<tool_call>`. | [five raw live captures](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-02-see-stream.json), [asserted result](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-02-see-result.json) |
| 03. Composer unlock | **Pass.** The first image's later screenshot was too late for the 2-second criterion, so a second See used the third and final image. Read-only polling caught a placeholder with two images and disabled controls, then the very first three-image frame **363 ms later** with textarea and Generate enabled. The short Do submitted after the first See was also accepted. | [preimage and first-image polling](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-03-unlock.json), [asserted result](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-03-result.json), [accepted Do in transcript](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-03-next-do-db.json), [post-Do UI](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-03-next-do-ui.json) |
| 04. See placement | **Pass.** First See's `$0.0685 · 10.0 s` image is attached to the earlier reply, its tooltip includes "See turn total", and that reply's `$0.0244` text cost did not change. | [asserted result](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-04-see-placement.json), [DOM and tooltip](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-03-unlock-ui.json), [image/turn rows](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-04-see-db.json) |
| 05. Retry | **Pass.** Last reply's replacement displays `$0.0181`, matching $0.018132 text usage including the $0.008262 earlier attempt; tooltip says "Includes $0.0083 from earlier attempts (Retry)." | [asserted result](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-05-retry.json), [prior UI](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-05-before-ui.json), [replacement UI](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-05-retry-ui.json), [usage rows](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-05-retry-db.json) |
| 06. Restart | **Pass.** Restarted Tauri and reopened the story. Header `$0.2577`, both text costs, three image captions, and complete read-only DB snapshot are unchanged. | [asserted comparison](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-06-restart.json), [before DOM](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-before-restart-ui.json), [after DOM](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-06-restart-ui.json), [before DB](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-before-restart-db.json), [after DB](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-06-restart-db.json) |
| 07. Logs | **Pass.** The current app log scan found zero `[ERROR]`/`panicked` matches; current WebView error entries were zero. | [log counts](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-07-logs.txt), [asserted result](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-07-result.json) |

All F4 checks have saved assertion results and primary raw evidence. The first delayed unlock screenshot was not counted as proof; the third image was reserved for the transition capture. The strict caps then prevented any more model requests. The previous project `pnpm tauri dev` process holding port 1420 was stopped and the clean branch build started afresh. The normal-ID app was stopped once QA completed.

## Calls, Costs And Commits

[Measured per-source spending](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/f4-spending.json): 10 F1 Do probes + 6 F2 See probes + 8 app text calls = **24 text calls** (cap 30); **3 images** (cap 3). Probe text $0.072566; app text $0.052202; app images $0.2054725. **Total $0.3302405**, within the approximate $0.35 budget. No further image or model call occurred after the third image.

Each production/test count below came from staged `git diff --numstat` and appears in its commit message; the older six are also detailed in [the prior QA report](file:///C:/Users/User/Documents/GitHub/story-llm/docs/report/turn-costs.md).

| Step | Commit | Production | Tests |
| --- | --- | ---: | ---: |
| T0 | `06c2af0` | +61/-61 | +23/-23 |
| T1 | `ca493dd` | +114/-23 | +144/-12 |
| T2 | `70b22fe` | +71/-2 | +49/-0 |
| T3 | `4163c53` | +68/-12 | +33/-10 |
| T4 | `d5d2e96` | +12/-1 | +0/-0 |
| Fix: early completion | `fdb8136` | +13/-1 | +57/-4 |
| See text suppression | `d71eb92` | +2/-0 | +0/-0 |
| Text cost beside reply | `1747fb4` | +4/-3 | +0/-0 |
| Text cost placement | `9755af5` | +8/-7 | +0/-0 |
| F2 rewording | `34a1ff3` | +3/-3 | +0/-0 |
| F3 Fix: early error | `edf77f8` | +21/-7 | +43/-0 |

The `fdb8136` fix handles `narration-done` arriving before IPC returns the stream id; otherwise See/Retry remains busy. The `edf77f8` fix handles the analogous early `narration-error`; otherwise the composer remains locked and a Retry retains a phantom replacement stream. Both failure modes have reproducing frontend tests.

Before each F2/F3 commit, `rg -n "ALTER TABLE|pragma_table_info|table_info|migration" src-tauri/src` returned only test-module matches in `shared/db.rs` and `turn/retry.rs`. A [per-commit historical migration audit](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs-finish/migration-audit.json) likewise confirms test-only matches for **all 11 branch commits**. There is no app schema migration or startup-upgrade code in this plan.

**Follow-ups:** F1 observed no normal-turn tool-text leaks, so the explicitly excluded narration guard was not added. Neither images-between-paragraphs nor the unrelated zero cache-write display was changed. The prompt's six See trials still included one text leak; the shipped See stream filter is the actual protection. No secrets file was printed or stored in evidence, no database rows were edited by QA, and no remote branch was pushed.
