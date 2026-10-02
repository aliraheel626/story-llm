# Turn And Image Cost QA

Branch `turn-costs` was created from clean `main` at `cab909d`. **Local `main` was not fast-forwarded**: T6-03 lacks two saved in-flight timer readings and the three-image spending cap is exhausted. The branch is clean at `fdb8136`, unpushed and without a PR. The app was stopped and the QA story `a42c228e-870a-4fc7-a02f-7c66535e9d66` was left for inspection. [Integration decision](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t7-main.txt) and [machine-readable QA gate](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-summary.json).

QA used **7 text model calls and 3 images**, within the 10/3 cap. Exact spent total was **$0.250857**: text $0.045552 and images $0.205305. The header displayed `$0.2509`, unchanged after erasure. [Exact backend totals](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-06-usage.json), [formatted totals comparison](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-06-totals.json).

## T5: Manual Database Change

**Pass.** With no matching app or dev process running, the old `usage_records` table and its indexes (`idx_usage_story` and its SQLite autoindex) were dropped by hand. `PRAGMA integrity_check` returned `ok`, foreign keys were clean, and the story and settings row counts were preserved. The new build recreated the table with `turn_id`, `image_asset_id`, `duration_ms`, `earlier_attempt`, and `idx_usage_turn`. Old usage history was discarded as approved. No startup migration code was added, and `secrets.json` was never opened, copied, deleted or overwritten. Evidence: [process inventory](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t5-processes.txt), [before/after table and integrity probe](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t5-drop.txt), [recreated schema](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-01-schema.json), [key presence only](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t7-main.txt).

## T6: Running-App Checks

| Check | Result | Saved Evidence |
| --- | --- | --- |
| 01. Schema | **Pass.** Four ordered columns and the turn index exist; usage rows and migration keys started at zero; integrity and foreign keys clean. | [schema](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-01-schema.json) |
| 02. Text turn | **Pass.** The visible `Turn $0.0124` equals the two rows attributed to the turn ID from `list_transcript_entries` ($0.012372 before formatting); the header is at least that amount. | [checked result](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-02-text-turn.json), [rows](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-02-db.json), [logical transcript](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-02-transcript.json), [DOM text](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-02-ui.json) |
| 03. See image and timer | **Failed / no live timer evidence.** All three images have correctly matched cost/duration caption lines and a `See turn total` tooltip. The tauri-pilot wait returned only after the placeholder was gone on the first and third attempts; the second See likewise finished before a saved in-flight reading. No two timer labels were saved at least one second apart. After three images, attempts four and five were forbidden by the hard spending cap; this check cannot be marked passed. | [failed-check summary](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-03-see.json), [first image rows](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-03-image1-db.json), [first caption](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-03-image1-ui.json), [third image rows](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-03-image3-db.json), [three captions](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-03-image3-ui.json), [post-See disabled composer](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-03-stuck-ui.json) |
| 04. Retry | **Pass.** The replacement turn shows `$0.0221`: $0.011518 for its own attempt plus $0.010578 moved from the earlier one. Its tooltip says so; the header increased from `$0.2393` to `$0.2509`. | [checked result](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-04-retry.json), [before rows](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-04-before-db.json), [after rows](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-04-after-db.json), [tooltip](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-04-after-ui.json), [breakdown command](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-04-breakdown.json) |
| 05. Restart | **Pass.** Turn lines, image captions, usage rows and header text are identical after restarting and reopening the story. | [comparison](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-05-restart.json), [DOM](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-05-restart-ui.json), [rows](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-05-restart-db.json) |
| 06. Totals | **Pass.** Visible reply lines sum to `$0.0345`, below the header `$0.2509`; the header matches `get_story_usage.total_cost_usd` ($0.250857) after formatting. See turns have captions, not reply lines, and erased/failed work may remain in the header. | [comparison](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-06-totals.json), [backend response](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-06-usage.json) |
| 07. Erase | **Pass.** The final Do reply and its turn line disappeared, but all billed usage rows and the header total remained. | [comparison](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-07-erase.json), [page](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-07-after-ui.json), [rows](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-07-after-db.json) |
| 08. Logs | **Pass.** No `[ERROR]`, `panicked`, or current WebView error entries were found. | [log scan](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-08-logs.txt) |

Image settings were already enabled and showed key availability, recorded only as booleans in [setup evidence](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-setup-image.json). Illustration was temporarily disabled *through the story's UI* once three images had been generated, to keep the later Retry within the image cap, then restored through the UI without additional model calls. Evidence: [disabled state](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-03-image-cap-protection.json), [restored state](file:///C:/Users/User/AppData/Local/Temp/codex/turn-costs/t6-04-tools-restored.json).

## Commits, Checks And Fix

Each commit message states additions/deletions measured with staged `git diff --numstat`; `#[cfg(test)] mod tests` and `*.test.mjs` are test lines. The required Rust tests, warning-free Clippy, frontend tests and TypeScript checks passed after every commit. Before each commit, the startup-migration grep (`ALTER TABLE|pragma_table_info|table_info|migration`) had hits only inside `db.rs` and `retry.rs` test code, never production startup code.

| Step | Commit | Production | Tests |
| --- | --- | ---: | ---: |
| T0 | `06c2af0` | +61/-61 | +23/-23 |
| T1 | `ca493dd` | +114/-23 | +144/-12 |
| T2 | `70b22fe` | +71/-2 | +49/-0 |
| T3 | `4163c53` | +68/-12 | +33/-10 |
| T4 | `d5d2e96` | +12/-1 | +0/-0 |
| Fix | `fdb8136` | +13/-1 | +57/-4 |

**Fix `fdb8136`:** After the third See finished in the database, the composer stayed disabled until restart. A completion event can arrive before submit/retry IPC returns the stream ID; the frontend discarded the event and subsequently installed a stream that never cleared. Unit tests reproduced this ordering for See and Retry. The fix replays an early completion once its stream is installed. All four checks passed after the Fix. The already-stuck QA UI was restarted before continuing. A new in-app See after this fix was not possible within the three-image cap, so neither the race fix nor the live timer is claimed as re-verified in-app.

## Choices And Boundaries

- The `run` skill was not available, so the normal-identifier build used tracked `pnpm tauri dev` and tauri-pilot via PowerShell. DOM captures only read the page; SQL probes used SQLite `mode=ro`. No probe edited DOM, application state or database rows.
- The first text-line CLI selector matched the header Total rather than the turn span; it was corrected with a read-only DOM capture. For the timer, two different wait/capture strategies were tried, but there is no saved pair of live readings. The three-image cap takes precedence over the suggested five-attempt ceiling.
- The user's existing usage history was deliberately discarded by the approved T5 table drop. The database and `secrets.json` still exist, and the QA story was intentionally left in place for inspection. No push or fast-forward occurred because T6-03 did not pass.
- Refreshing a timed-out image's late cost immediately and adding a cost chart remain outside this plan.
