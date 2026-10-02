# Startup cleanup: remove the `com.dungeon.app` importer and the old migrations, then QA with tauri-pilot

## Context

The review of the transcript-finish run (2026-09-29) found one latent bug on `main` (`6298e7b`). The user then asked to fold in removing the old startup migrations.

### 1. The legacy importer is broken

**What it does:** `init_pool` in `src-tauri/src/shared/db.rs` still copies the old app's data. This happens when all of these are true:
- `com.story-llm.app` has no database
- the folder is named `com.story-llm.app`
- the folder holds anything other than only `secrets.json`
- `%APPDATA%\com.dungeon.app\dungeon.sqlite3` exists

**Why it's broken:** that database is from before the rename, and nothing renames `ledger_entries` at startup any more. Every import therefore leaves the old stories with their entries in `ledger_entries`, beside an empty `transcript_entries`, so the stories look empty. Codex's own evidence shows it: `%TEMP%\codex\transcript-finish\z4-01-attempt1.json`.

**Codex's guards:** `108aad1` added `secrets_are_only_app_data` and `6298e7b` added the `.legacy-import-pending` marker. They only narrow *when* the import runs. It still runs if:
- the whole `com.story-llm.app` folder is deleted, so there's no `secrets.json`
- a stray file sits next to `secrets.json` while the database is missing

### 2. The old startup migrations are dead weight

`run_migrations` creates the tables, then runs 12 upgrade steps. Each one converts a database shape that no longer exists anywhere:

| Upgrade step (at `6298e7b`) | What it upgrades |
|---|---|
| `migrate_ledger_schema` (763) | `timeline_entries` → `ledger_entries`: two renames ago, and it recreates `idx_ledger_*` indexes |
| `migrate_roll_needed_v1` (541) | old dice-roll payloads with no `needed` field |
| `migrate_image_blobs_v1` (486) | image files on disk → `image_blobs` rows |
| `ensure_transcript_turn_column` (588) | adds `turn_id`, which is already in `CREATE TABLE` |
| `ensure_turn_attempt_column` (573) | adds `attempt`, which is already in `CREATE TABLE` |
| `migrate_ledger_retention_settings` (804) | the `timeline_retention` key → `ledger_retention`, which no code reads any more |
| `migrate_narrator_memory_settings` (1025) | the old `narrator_memory` settings key |
| `migrate_author_notes` (1116) | author's notes from transcript events → story settings |
| `migrate_narrator_tools` (843) | old variant, selection and dice rows, and the old tool settings |
| `migrate_story_injection_v1` (1179) | the global `context_injection` key → per-story settings |
| `migrate_turns_v1` (608) | backfills `turns` for stories from before turns existed |
| `DROP INDEX IF EXISTS idx_turns_one_pending` (477) | an index that turn transactions removed |

**Why they're safe to delete:**
- The user's only database was created fresh by the transcript-finish build, which already has the current schema. Every step above is a no-op on it.
- The only thing each step still does on it is check its `migration_*` marker row.
- L4 proves this by comparing a fresh database made without the migrations to the user's database.

**Their size:** about 750 production lines and about 1,150 test lines, which is most of `db.rs`.

### The user's decisions (2026-09-29)

1. **Remove the importer entirely** instead of guarding it, and **remove the old migrations.** Old data doesn't matter, and nobody else uses the app.
2. **Delete the old app folder** `%APPDATA%\com.dungeon.app\` (about 15 MB of old databases, `.bak` copies and images).
3. **Run to the end without the user.** Nothing in this plan asks the user anything or waits for them.
4. **Fix failures instead of stopping.**
5. **QA with tauri-pilot** in the real app.

### Outcome

- `init_pool` only opens or creates `story-llm.sqlite3`, and never looks at another folder.
- Startup creates the current schema and does nothing else: no upgrade steps and no marker rows. `run_migrations` is renamed `create_schema`.
- Nothing on disk is left for an importer to find.
- The user's database, stories and `secrets.json` are unchanged, apart from the removed `migration_*` marker rows.
- `main` is fast-forwarded (not pushed).
- `db.rs` shrinks from 3,055 lines to about 600.

### Out of scope

Read-time compatibility code for old data shapes stays, for example:
- in `transcript/history.rs`: "legacy author note events are inert", and the legacy dice-roll preference
- the legacy effect kinds in `TurnActivity.tsx`
- the unknown legacy reasoning effort in `stories/settings.rs`
- `create_story_in_pool` stripping `attributes_enabled` and `dice_mode`
- `settings::refresh_missing_capabilities`

These aren't startup migrations. They're a separate, later cleanup; list them in the report (`docs/report/startup-cleanup.md`) as candidates.

## Ground rules

- **Run the whole plan in one go, without asking the user anything.** The user has approved every step, including:
  - deleting `%APPDATA%\com.dungeon.app\`
  - deleting the `migration_*` rows from the user's `settings` table
  - fast-forwarding `main`

  Where something is unclear, choose, and record the choice in the report (`docs/report/startup-cleanup.md`).
- **Fix, don't stop.** When a check fails, at any step:
  1. Find the cause and fix it on the branch, with a unit test when the bug can be reproduced in one. Commit it as "Fix …", with its line counts and a one-line cause.
  2. Rerun the four checks below, then rerun the failed check and every check after it.
  3. Repeat up to **5 attempts per check**.
  4. If a check still fails after 5, record it as failed with what you tried, carry on with the remaining checks, and **don't fast-forward `main`** (L7).
  - Fixes stay within what the failing check needs. **Never fix a failure by putting a migration back.** If the user's database really needs a change, make it once by hand with the app stopped, the way L5 does, and record it.
  - If the problem is in the environment (a stuck process, a locked file, a port in use), fix the environment.
- **Allowed to delete:**
  - `%APPDATA%\com.dungeon.app\` and `%LOCALAPPDATA%\com.dungeon.app\` (if it exists), entirely
  - the `migration_*` rows in the user's `settings` table (L5)
  - the throwaway database and `qa-trigger.txt` from L4
  - the QA story from L6
- **Never delete:**
  - `%APPDATA%\com.story-llm.app\secrets.json`
  - the user's `story-llm.sqlite3` / `-wal` / `-shm` (L4 moves them aside and back)
  - any of the user's stories, apart from the QA story
  - git history, or `origin`
- **Never push.** Never print, log, open or copy any `secrets.json` or `secrets.json.bak`, in either folder. List them by name and size only.
- **Branch:** create `startup-cleanup` from `main` at `6298e7b`. If `main` has moved, branch from the current `main` and record its commit.
- **Before starting:** `git status` must be clean. If the only change is the `.gitignore` line `docs/`, commit it first as "Ignore local docs", and record that in the report.
- **One commit per code step** (L1, L2, L3, plus any "Fix …" commits). Each message states its production and test line counts from `git diff --numstat`.
- **After every commit:**
  - `cargo test --manifest-path src-tauri/Cargo.toml`
  - `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` with zero warnings
  - `node --test src/features/story/store.test.mjs src/features/transcript/replacement.test.mjs src/features/stats/store.test.mjs`
  - `npx tsc --noEmit`
- **The report** goes in this repository at `docs/report/startup-cleanup.md`.
  - Create `docs/report/` if it's missing.
  - `docs/` is git-ignored, so the report is never committed.
  - It links each evidence file by its full path.
- **Evidence files** go in `%TEMP%\codex\startup-cleanup\`. Every QA check saves its output to a file. A check without a saved file is "no evidence", never a pass.
- **QA probes read; they don't fake results.**
  - Drive the app like a user: real clicks and typing through tauri-pilot.
  - Database reads are read-only `SELECT`s and `PRAGMA`s, except L5's single `DELETE`.
  - Never edit the DOM, app state or database rows to make a check pass.
- **Model spending cap:** at most **2 text model calls** in total, including fix reruns. No image generation.
- **tauri-pilot notes from earlier runs:**
  - The pipe is `\\.\pipe\tauri-pilot-com.story-llm.app`. Drive it from **PowerShell**; Git Bash mangles the pipe path.
  - PowerShell 5.1 splits double-quoted arguments that contain spaces. Use here-strings, or prefix selectors like `[aria-label^=New]`.
  - Start the app with the `run` skill if it's available, otherwise `pnpm tauri dev`.

---

## L1. Remove the importer (`src-tauri/src/shared/db.rs`)

Line numbers are for `6298e7b`.

### Production code

1. **`init_pool` (lines 63–81):** delete the whole `if !path_exists(&db_path)? && … { … }` block. The function becomes:
   ```rust
   pub fn init_pool(app_data_dir: &Path) -> AppResult<Pool> {
       fs::create_dir_all(app_data_dir)?;
       let db_path = app_data_dir.join("story-llm.sqlite3");
       let manager = SqliteConnectionManager::file(db_path).with_init(|conn| {
       // … unchanged from here …
   ```
2. **Delete lines 96–344,** from `fn path_exists` through the blank line after `rebase_image_paths`. These are 7 functions that nothing else uses: `path_exists`, `secrets_are_only_app_data`, `migrate_app_data`, `copy_missing_files`, `copy_missing_file`, `files_match` and `rebase_image_paths`.
   - First confirm with `rg -n "path_exists|files_match|copy_missing_file|rebase_image_paths|migrate_app_data|secrets_are_only_app_data" src-tauri/src` that every hit is inside that range or in a test that L1 deletes.
3. **Imports:**
   - `use std::fs::{self, File, OpenOptions};` becomes `use std::fs;`
   - `use std::io::{self, Read};` becomes `use std::io;`, which L2 removes entirely
   - delete the `#[cfg(unix)] use std::os::unix::fs::PermissionsExt;` pair
   - `use std::path::{Component, Path, PathBuf};` becomes `use std::path::{Path, PathBuf};`
   - Let the compiler and Clippy confirm there's nothing unused and nothing missing.

### Tests (same file)

4. **Delete lines 1468–1755,** which hold 5 importer tests:
   - `app_data_migration_preserves_wal_images_and_secrets_without_reimporting`
   - `snapshot_reconciliation_copies_late_image_and_reuses_collision_file_on_retry`
   - `migrated_secrets_keep_restrictive_permissions` (unix only)
   - `preexisting_new_db_is_never_replaced_or_merged`
   - `failed_file_copy_retries_before_publishing_new_db`
5. **Replace `retained_secrets_only_start_a_fresh_transcript_database`** (lines 1309–1331) with a new test, `neighbouring_dungeon_app_data_is_never_imported`.
   - **Keep** the `legacy_app_db` helper (lines 1450–1466); the new test uses it.
   - **Setup:** `parent` is a temp dir. `legacy_app_db(&parent)` creates the old database with story `legacy`. Then add `com.dungeon.app\images\scene.png` and `com.dungeon.app\secrets.json` with placeholder bytes.
   - **Case A: the folder is missing.** `com.story-llm.app` doesn't exist at all. This is the case Codex's guard still imported.
     - Call `init_pool(&parent.join("com.story-llm.app"))`.
     - Assert:
       - 0 stories
       - `transcript_entries` exists and `ledger_entries` doesn't
       - no `images` folder and no `secrets.json` in the new folder
       - every file in the new folder starts with `story-llm.sqlite3`
   - **Case B: a stray file.** In a second `parent`, create `com.story-llm.app\` holding `secrets.json` and `notes.txt`, then call `init_pool`.
     - Assert 0 stories, no `ledger_entries`, and both files unchanged.
   - Drop the connections, and remove both temp dirs at the end.

### Line estimate and commit

| | Production | Tests |
|---|---|---|
| `init_pool` block | −19 | |
| Seven helper functions | −249 | |
| Imports | −2 | |
| Five importer tests | | −288 |
| Replaced test (23 → about 40 lines) | | +15 to +20 |
| **Total** | **about −270** | **about −270** |

**Commit:** "Remove the com.dungeon.app legacy importer".

---

## L2. Remove the old startup migrations (`src-tauri/src/shared/db.rs`)

Line numbers are for `6298e7b`, since L1 shifts them. **Find every item by name.**

### Production code

1. **Rename `run_migrations` to `create_schema`,** and update its one call in `init_pool`. Its doc comment: "Creates the current schema on a fresh database. There are no upgrade steps: a database from an older build is deleted, not migrated."
2. **Inside it:**
   - Delete the first line, `migrate_ledger_schema(conn)?;`.
   - In the `execute_batch` schema, add after `idx_transcript_kind`:
     ```sql
     CREATE INDEX IF NOT EXISTS idx_transcript_turn ON transcript_entries(turn_id);
     ```
     Today this index is created only by `ensure_transcript_turn_column`.
   - Delete the 10 calls after the batch (`migrate_roll_needed_v1` … `migrate_turns_v1`), and `conn.execute_batch("DROP INDEX IF EXISTS idx_turns_one_pending;")?;`.
   - **Keep** the `auto_vacuum` block and `PRAGMA incremental_vacuum;` unchanged. The `VACUUM` is what turns on incremental vacuum for a new database: the pool switches to WAL mode before the tables are created, which most likely fixes the file header, so a plain `PRAGMA auto_vacuum` at that point would be ignored. Don't try to simplify this block.
3. **Delete 11 functions:**
   - `migrate_image_blobs_v1`, `migrate_roll_needed_v1`, `ensure_turn_attempt_column`, `ensure_transcript_turn_column`, `migrate_turns_v1`, `migrate_ledger_schema` and `migrate_ledger_retention_settings` (lines 486–815, one block)
   - `migrate_narrator_tools`, `migrate_narrator_memory_settings`, `migrate_author_notes` and `migrate_story_injection_v1` (lines 843–1251, one block)
   - **Keep `seed_player_entity`** (between the two blocks); story creation uses it. Change its doc comment to: "Idempotent on name: story creation calls this inside its own transaction."
   - **Check:** `rg -n "run_migrations|migrate_|ensure_.*_column|migration_" src-tauri/src` shows no hits.
4. **Imports:** remove what's now unused: likely `rusqlite::OptionalExtension`, `std::collections::HashMap`, `std::io`, `crate::features::transcript::{model::kind, repository}` and `crate::features::stories::settings::NarratorToolSettings`. Let the compiler and Clippy decide.
5. **`features/transcript/model.rs`:** `kind::DICEROLL_SETTINGS_CHANGED` was used only by `migrate_narrator_tools`.
   - If Clippy reports it unused, delete the constant.
   - Remove its mentions from the comments at `model.rs:76` and `transcript/filter.rs:36`, keeping the rest of each comment's meaning.
   - The frontend's legacy handling stays (out of scope).

### Tests (same file)

6. **Delete 18 migration tests,** by name. The line ranges are for `6298e7b`, including each block's trailing blank line.

   | Lines | Tests |
   |---|---|
   | 1389–1449 | `image_migration_imports_files_once_and_keeps_backups` |
   | 1756–2039 | `roll_needed_migration_backfills_once_without_touching_other_payloads`, `legacy_stories_reset_tools_once_and_new_stories_default_on`, `migration_materializes_selected_edit_and_discards_legacy_rolls_without_repeating` |
   | 2125–2796 | `existing_transcript_table_gains_nullable_turn_reference_and_index`, `existing_turns_table_gains_attempt_column_with_zero_default`, `existing_transcript_data_and_foreign_keys_survive_upgrade`, `failed_ledger_schema_upgrade_rolls_back_the_rename`, `ambiguous_ledger_schema_is_not_overwritten`, `legacy_retention_key_does_not_replace_newer_ledger_preference`, `legacy_note_and_memory_settings_migrate_once`, `empty_narrator_memory_migration_is_recorded`, `empty_author_note_migration_is_recorded`, `story_injection_migration_preserves_muted_notes_and_scoped_mode_once`, `story_injection_migration_rejects_invalid_global_entity_mode`, `legacy_memory_imports_once_without_overwriting_newer_preferences` |
   | 2886–3023 | `turns_backfill_assigns_exact_ownership_and_status_once`, `startup_preserves_legacy_pending_rows_and_drops_old_index` |

   "New stories default on" is already covered by `story_creation_validates_draft_and_seeds_canonical_player` in `stories/repository.rs`.
7. **Keep:**
   - L1's new test
   - `blocking_returns_work_value`, `blocking_preserves_work_error` and `blocking_write_finishes_after_turn_commit`
   - `seed_player_entity_respects_existing_case_insensitive_name_and_no_attributes`
   - `fresh_database_has_transcript_table`
   - `canonical_name_uniqueness_is_enforced_case_insensitively_across_connections`
   - `entity_names_are_unique_case_insensitively_per_story`
   - `with_transaction_commits_success_and_rolls_back_errors`
8. **`fresh_database_has_transcript_table`:**
   - Keep the `idx_transcript_turn` assertion; it now proves the schema batch creates the index.
   - Drop the `idx_turns_one_pending` assertion.
   - Add:
     - `PRAGMA auto_vacuum` is `2`
     - `SELECT COUNT(*) FROM settings WHERE key LIKE 'migration%'` is `0`
     - calling `init_pool` a second time on the same directory succeeds and leaves the table list unchanged

### Line estimate and commit

| | Production | Tests |
|---|---|---|
| Calls in `run_migrations`, and the index line | −12, +1 | |
| First block of 7 functions | −330 | |
| Second block of 4 functions | −409 | |
| Imports and the dead constant | about −6 | |
| 18 tests | | about −1,155 |
| `fresh_database_has_transcript_table` changes | | about +10 |
| **Total** | **about −755** | **about −1,145** |

**Commit:** "Remove the old startup migrations; a fresh schema is the only path".

---

## L3. Update the README's "Local data" section

`README.md` lines 93–99 describe the importer. Replace that paragraph with:

> The SQLite database (`story-llm.sqlite3`, which also stores generated images) and the API-key store (`secrets.json`) live in the operating system's Tauri application-data directory for `com.story-llm.app`. The app creates the current schema on a fresh database and never upgrades an older one: after a schema change, delete the `story-llm.sqlite3*` files and keep `secrets.json` to keep the API key. Do not commit secrets or local application data.

- Images are served from the `image_blobs` table (`features/transcript/attachments.rs`), so "which also stores generated images" is correct. Confirm it.
- Then run `rg -n -i "dungeon|migration" README.md src src-tauri/src`. Leave out `docs/`, which holds this plan.
  - Only the `legacy_app_db` test helper may still mention "dungeon".
  - Remaining "migration" hits are fine only in the read-time legacy code listed as out of scope.
  - Save the output as `l3-grep.txt`.

**Line estimate:** docs about −3. **Commit:** "README: no importer, no upgrades; delete the database after a schema change".

---

## L4. QA: no import, and the fresh schema matches the user's database

This runs **before** L5, so the old folder is still there to tempt the importer.

1. **Stop the app.**
   - Stop every `story-llm.exe` process, and every `tauri dev` / `vite` process started from this repository.
   - Save the process list from before and after as `l4-processes.txt`.
2. **Record the starting state.**
   - **Folders,** with names, sizes and LastWriteTime only, no contents:
     - `Get-ChildItem -Recurse -Force` of `%APPDATA%\com.story-llm.app` and `%APPDATA%\com.dungeon.app`
     - whether `%LOCALAPPDATA%\com.dungeon.app` exists
     - Save as `l4-before.txt`.
   - **The user's database, read-only,** made by the old code:
     - `SELECT COUNT(*) FROM stories`, `SELECT id FROM stories ORDER BY updated_at DESC`, and the row counts of every table
     - `SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name`
     - `SELECT key FROM settings ORDER BY key` (key names only)
     - `PRAGMA auto_vacuum`
     - Save as `l4-user-db.json`. L6 compares against this.
3. **Set up the trigger:**
   1. Move the user's `story-llm.sqlite3`, `story-llm.sqlite3-wal` and `story-llm.sqlite3-shm` (whichever exist) to `%TEMP%\codex\startup-cleanup\db-aside\`. **Move all three together, with the app stopped.**
   2. Create an empty `%APPDATA%\com.story-llm.app\qa-trigger.txt`.

   With `secrets.json` plus `qa-trigger.txt` and no database, `6298e7b` would import the 3 old stories.
4. **Start the branch build with tauri-pilot.** Probe, and save everything as `l4-fresh.json`:
   - **No import:**
     - `transcript_entries` exists, `ledger_entries` doesn't, and there are 0 stories.
     - The story list in the UI shows none of the old stories.
     - `com.story-llm.app` has no `images` folder, no `.legacy-import-pending`, and no `.story-llm-migration-*` files.
   - **Old folder untouched:** `%APPDATA%\com.dungeon.app` matches `l4-before.txt`, with the same names, sizes and LastWriteTime, including `dungeon.sqlite3-shm`. Nothing opened it.
   - **Same schema:** the fresh database's `sqlite_schema` list has exactly the same `type`, `name` and `tbl_name` rows as the user's database in `l4-user-db.json`, and the same `sql` after collapsing whitespace.
     - Any difference is a failure to fix in L2's schema batch. **Never fix it by putting back a migration.**
     - The one allowed difference: `CREATE INDEX IF NOT EXISTS` versus `CREATE INDEX` text, if SQLite keeps it.
   - **No marker rows:** there are no `migration_%` keys in `settings`, and `PRAGMA auto_vacuum` is `2`.
5. **Restore the user's database:**
   1. Stop the app.
   2. Delete the throwaway `story-llm.sqlite3*` files from step 4, and `qa-trigger.txt`.
   3. Move the three files back from `db-aside\`.
   4. Compare the names and sizes with `l4-before.txt`. They must match exactly.
   - Save as `l4-restore.txt`.
   - If the restore fails, fixing it takes priority over everything else in this plan: the user's database must be back before L5.

---

## L5. Delete the old app folder and the marker rows

With the app still stopped:
1. **Delete** `%APPDATA%\com.dungeon.app\` recursively, and `%LOCALAPPDATA%\com.dungeon.app\` if it exists.
2. **In the user's database, run once:**
   ```sql
   DELETE FROM settings WHERE key LIKE 'migration%';
   ```
   This is the only write to the user's database in this plan. Record the key names deleted (from `l4-user-db.json`) and the row count affected.
3. **Check:**
   - neither `com.dungeon.app` folder exists
   - `secrets.json` in `%APPDATA%\com.story-llm.app\` has the same size and LastWriteTime as in `l4-before.txt`
   - `PRAGMA integrity_check` returns `ok`
   - `PRAGMA foreign_key_check` returns no rows
4. Save as `l5-cleanup.txt`.

No commit, since nothing in the repository changes.

---

## L6. QA: the normal app after the cleanup

Start the branch build with tauri-pilot, on the user's restored database.

1. **The user's data is intact.**
   - The story count and ids, and every table's row count, equal `l4-user-db.json` (only `settings` is lower, by L5's deleted rows).
   - The schema list is unchanged, and no `migration_%` key has come back.
   - If there's at least one story, open the newest one. The number of rendered transcript entries must match what `list_transcript_entries` returns for it.
   - Save as `l6-01-data.json`.
2. **Settings:** `get_text_model_settings` reports `has_api_key: true`. Record the boolean only.
   - If it's `false`, record "no API key", and skip check 3 as "not possible: no API key".
   - Save as `l6-02-settings.json`.
3. **One turn in a new story:**
   1. Click "+ New story".
   2. In **Narrator Tools**, turn off "Illustrate scenes".
   3. Submit the "Do" action "Look around the room."
   - **Pass if:**
     - the narration finishes
     - `transcript_entries` has the player message and the narration with one `turn_id`
     - the turn is `complete`
     - `usage_records` has a `narration` row with a non-null `cost_usd`
     - the header total is above $0.00
   - Uses 1–2 of the 2 allowed model calls, counting the auto-title. Save as `l6-03-turn.json`.
4. **Restart:**
   - Stop and start the app.
   - **Pass if:**
     - the QA story and its entries are unchanged
     - the user's stories are all still there
     - no `com.dungeon.app` folder and no `migration_%` key has appeared
   - Save as `l6-04-restart.json`.
5. **Clean up:**
   - Delete the QA story through the UI.
   - **Pass if** its rows in `turns`, `transcript_entries` and `usage_records` go to 0, and the story count equals `l4-user-db.json` again.
   - Save as `l6-05-delete.json`.
6. **Logs:**
   - Scan the app's log output for L4 and L6 for `ERROR` and `panicked`.
   - **Pass if** there's no error the checks didn't expect.
   - Save as `l6-06-logs.txt`.

Stop the app at the end.

---

## L7. Fast-forward `main`

- **Only if every L4 and L6 check passed** (after fixes), and L5 succeeded:
  - `git checkout main`
  - `git merge --ff-only startup-cleanup`
  - save `git log --oneline -5` as `l7-main.txt`
  - **Don't push.**
- **If `main` has moved** so the fast-forward isn't possible, don't rebase. Instead:
  1. Merge `main` into the branch.
  2. Rerun the four checks and L6 checks 1 and 4.
  3. Then fast-forward.
- **If a check failed after 5 attempts:** leave `main` as it is, and say which check failed.
- Leave the working tree clean, with `main` checked out.

---

## Final message and the report, `docs/report/startup-cleanup.md` (the only things the user reads)

- Whether `main` was fast-forwarded, and to which commit.
- Each commit with its measured production and test line counts, compared to the L1, L2 and L3 estimates, and `db.rs`'s line count before and after.
- Each L4 and L6 check: pass or fail, with its evidence file.
- **Proof of five things:**
  - the trigger case imported nothing
  - the fresh schema equals the user's database schema
  - `%APPDATA%\com.dungeon.app` was untouched until L5, then deleted
  - the user's stories and `secrets.json` are exactly as before
  - which `migration_*` rows were deleted
- The model calls used, and their cost from the header.
- Each "Fix …" commit: what failed, the cause, and the fix.
- The out-of-scope read-time legacy code you noticed, as candidates for a later cleanup.
- Anything done that the plan didn't name.
