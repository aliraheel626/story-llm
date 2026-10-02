# Startup Cleanup Report

Local `main` was fast-forwarded to **`cab909d`** from `3a78d9f`. It is checked out and clean, 14 commits ahead of `origin/main`; nothing was pushed or opened as a PR. The ignored report is not committed. [Final commit history](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l7-main.txt) and [integration gate](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l7-gate.json).

`init_pool` no longer inspects `com.dungeon.app`. `create_schema` has no upgrade steps or migration markers and still enables incremental auto-vacuum. `src-tauri/src/shared/db.rs` fell from **3,055 to 611 lines**. The user's original database was moved aside and restored as the original DB/WAL/SHM triplet during QA. Only seven authorized marker rows were deleted; the user's story IDs, all other table row counts, and schema were preserved. `secrets.json` was never opened or copied: its name, size and nanosecond LastWriteTime are unchanged.

## Commits And Checks

Counts are additions/deletions measured with staged `git diff --numstat` (Rust `#[cfg(test)] mod tests` blocks and `*.test.mjs` count as tests). Each L1-L3 commit passed `cargo test` (170 Rust tests at L3), warning-free Clippy, the required Node suite (43 tests), and `npx tsc --noEmit`. No `Fix` commit was needed.

| Step | Commit | Production | Tests | Compared With Estimate |
| --- | --- | ---: | ---: | --- |
| Pre-step | `3a78d9f` Ignore local docs | +3/-0 | +0/-0 | The only pre-existing worktree change; committed before branching. |
| L1 | `ce5f7f9` Remove legacy importer | +3/-273 | +32/-309 | Production net -270 as estimated; tests net -277, near -270. |
| L2 | `8c00cbe` Remove old migrations | +11/-766 | +15/-1158 | Production net -755 and tests net -1143, matching the ~-755/-1145 estimates. |
| L3 | `cab909d` Update Local data docs | +6/-7 | +0/-0 | Docs net -1, rather than the ~-3 estimate. |

The [leftover-name search](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l3-grep.txt) finds `dungeon` only in the intentionally retained test helper and its test. `migration` remains in the required zero-marker assertion and a read-time retry test comment. Generated image bytes are stored in `image_blobs`; the README now says so.

## L4: Trigger And Restore

**Pass: no legacy import.** With the user's original DB/WAL/SHM safely [moved aside](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-move.txt), an empty `qa-trigger.txt` beside the retained key reproduced the condition the old importer would have followed. The app started with zero stories, `transcript_entries` only, no legacy images or importer marker, no `migration%` keys, and `auto_vacuum = 2. The UI listed no old stories. Evidence: [fresh comparison](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-fresh.json), [fresh database](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-fresh-db.json), [tauri-pilot UI](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-fresh-ui.json).

**Pass: same schema.** The fresh and user database have identical `(type, name, tbl_name)` schema rows and identical SQL after whitespace normalization and the permitted index `IF NOT EXISTS` normalization. Evidence: [original read-only database inventory](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-user-db.json), [fresh comparison](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-fresh.json).

**Pass: old folder untouched until L5.** All 25 inventoried old-folder entries, including `dungeon.sqlite3-shm`, retained their names, sizes and LastWriteTime while the new app ran. Only names and metadata were inspected, never secrets content. Evidence: [before inventory](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-before.txt), [during inventory](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-during.txt), [comparison](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-fresh.json), [stopped-process inventory](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-processes.txt).

**Pass: user's original triplet restored.** The app was stopped; only the throwaway database triplet and `qa-trigger.txt` were removed; the three original files were moved back from `db-aside`. Names and sizes, story IDs, row counts, full schema SQL, setting keys, and auto-vacuum agree with before the move. The `-shm` file's LastWriteTime changed during read-only SQLite access, but its size is unchanged; the plan requires name/size and database-content comparisons, all of which passed. Evidence: [restore record](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-restore.txt), [after inventory](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-after-restore.txt), [read-only restored DB](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-restored-db.json), [restore comparison](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l4-restore-check.json).

## L5: Authorized Cleanup

**Pass.** Both `%APPDATA%\com.dungeon.app` and `%LOCALAPPDATA%\com.dungeon.app` were deleted only after L4 proved the old folder had remained untouched. A single SQL `DELETE FROM settings WHERE key LIKE 'migration%'` removed exactly seven rows: `migration_author_notes_to_story_settings`, `migration_image_blobs_v1`, `migration_narrator_memory_split`, `migration_narrator_tools_stateless_roll_v1`, `migration_roll_needed_v1`, `migration_story_injection_v1`, and `migration_turns_v1`. `PRAGMA integrity_check` returned `ok`; `PRAGMA foreign_key_check` returned no rows. Evidence: [deleted folders](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l5-folders.txt), [deleted keys/count and integrity](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l5-cleanup.txt), [secret metadata match](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l5-secret-check.json).

## L6: Normal-App QA

| Check | Result | Evidence |
| --- | --- | --- |
| 01. User data | **Pass.** The original database had zero stories; all story IDs and other table counts match its baseline, with `settings` lower only by the seven deleted keys. Schema unchanged and no marker reappeared. Existing-story rendering was not applicable. | [comparison](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-01-data.json), [DB](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-01-db.json), [UI](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-01-ui.json) |
| 02. Key availability | **Pass.** `get_text_model_settings.has_api_key` is true; only that boolean was saved. | [settings](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-02-settings.json) |
| 03. One Do turn | **Pass.** Illustration was disabled through the UI. The player/narration rows share one complete turn, narration has reported cost, and the stats header showed a positive text amount (with zero image cost). | [checks and exact cost](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-03-turn.json), [read-only rows](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-03-turn-db.json), [tauri-pilot UI](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-03-turn-ui.json) |
| 04. Restart | **Pass.** The QA story and entries are byte-for-byte equal across restart, with no old folder or marker rows. | [checks](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-04-restart.json), [DB](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-04-restart-db.json), [UI](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-04-restart-ui.json) |
| 05. Delete QA story | **Pass.** Deleted through the UI; the QA story, its turns, transcript rows and usage rows are zero. The user's original zero-story baseline is restored. | [checks](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-05-delete.json), [QA row probe](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-05-deleted-db.json), [final DB](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-05-final-db.json), [UI](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-05-ui.json) |
| 06. Logs | **Pass.** No `[ERROR]`, panic, or current WebView error entries in the app log scan. | [log counts](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-06-logs.txt) |

The [machine-readable L6 result](file:///C:/Users/User/AppData/Local/Temp/codex/startup-cleanup/l6-summary.json) records all six passes. Exactly **two text model calls** were used, one narration and one title, with an exact recorded cost of **$0.015722** (header Text displayed `$0.0157`, Images `$0.00`); no image was generated. The app was stopped after QA.

## Deferred Read-Time Compatibility

These were explicitly out of scope and remain candidates for a later cleanup: legacy author-note events and dice-roll preference in `features/transcript/history.rs`; legacy effect kinds in `src/features/transcript/TurnActivity.tsx`; unknown old reasoning effort in `features/stories/settings.rs`; `create_story_in_pool` removing `attributes_enabled` and `dice_mode` in `features/stories/repository.rs`; and `settings::refresh_missing_capabilities` in `features/settings/text_model.rs`.

The pre-step `docs/` ignore entry was committed to clean `main` before branching, so `startup-cleanup` was based on `3a78d9f` rather than directly on `6298e7b`. The `run` skill was unavailable in this session; tracked `pnpm tauri dev` and tauri-pilot were used. No other code change or environment workaround was needed, and no fix or migration was reintroduced.
