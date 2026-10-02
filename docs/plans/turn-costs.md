# Per-turn cost, per-image cost, and an image timer, shown in the transcript

## Context

The header above the story (today `src/features/stats/StatsBar.tsx`; T0 renames it `usage/StoryHeader.tsx`) shows **story totals**: text, images, cached input, cache write, and the total. They come from `get_story_stats` (renamed `get_story_usage` in T0), which sums `usage_records` (`src-tauri/src/features/stats/`, becoming `usage/`).

**The user wants, in the transcript itself** (separate from the header totals):
1. **Under each reply, that turn's cost,** split into text and images, for example `Turn $0.0123 · text $0.0040 · images $0.0083`.
2. **Under each image, that image's cost and how long it took,** for example `$0.0390 · 14.2 s`.
3. **A live timer while an image is generating.**

**What's missing today:**
- `usage_records` has **no link to a turn or an image**. It was left out on purpose, so rows outlive rolled-back turns, retries and erased exchanges. Its columns are `id, story_id, kind, provider, model, response_id, input_tokens, output_tokens, cached_input_tokens, cache_write_tokens, cost_usd, created_at`.
- **Image timing isn't recorded anywhere.** `images/generation.rs::generate_from_description` makes the request inside a spawned task with a 120 s timeout. A timed-out image records its late cost through `usage::repository::record_late`.
- **The image's asset id is created after the cost is recorded:** `persist_and_store_image` makes the UUID. The cost therefore can't point at the image yet.

**How turns behave** (this decides the design):
- A normal turn creates one `turns` row. `submit.rs` calls `turns::create_turn` inside the turn transaction, and its entries carry that `turn_id`.
- **Retry deletes the old turn and makes a new one.** `turn/retry.rs::prepare_retry` calls `erase::remove_turn(old_turn)`, then `submit::run_turn` creates a new turn with a new narration entry. If the retry fails, its `TurnTx` rolls back, which restores the old turn.
- **A See turn** has a hidden player message and **no narration of its own.** Its image is attached to the previous narration entry (`ImageTarget.entry_id` = the prior narration, `turn_id` = the See turn).
- **Usage is buffered in `TurnTx`** (`turn/tx.rs`) and written at `commit()`, or after `ROLLBACK` (in `rollback()` and in `Drop`), by `flush_usage`.

**Module layout (decided 2026-09-29):**
- **Rename `stats` to `usage`,** and the header component to `StoryHeader` (step T0).
  - **Backend:** the module is named for the data it owns (`usage_records`), not for where the UI shows it. The UI's "header" is one reader of that data; the new per-turn lines in the transcript are another.
  - **Frontend:** `src/features/stats/` becomes `src/features/usage/`, and `StatsBar` becomes `StoryHeader`.
- **Per-turn costs live in `usage`, not in `turn`.** Each module keeps to its role:
  - **`turn`:** the turn transaction and its flow. It *labels* each cost with its turn while the transaction is open (`TurnTx`, step T1).
  - **`usage`:** owns every read and write of `usage_records`. That includes totals, per-turn sums, per-image costs, and moving a replaced turn's rows on retry.

  Putting the per-turn query in `turn` would split `usage_records` SQL across two modules.
- **Not one module per table.** Several features naturally span tables, for example `transcript` with `transcript_entries`, `turns` and `image_assets`. Splitting by table would scatter each feature. The useful part of that idea is kept as a rule: **each table's SQL lives in exactly one module.**

  | Module | Owns |
  |---|---|
  | `usage` | `usage_records` |
  | `transcript` | `transcript_entries`, `turns`, `image_assets`, `image_blobs` |
  | `stories` | `stories` |
  | `entities` | the entity tables |
  | `settings` | `settings` |

  Other modules call these modules' functions. Where existing code breaks this rule, leave it; this plan's new code follows it.

**Design:**
- **New columns on `usage_records`,** none of them foreign keys, so rows still outlive turns:
  - `turn_id TEXT`
  - `image_asset_id TEXT`
  - `duration_ms INTEGER`
  - `earlier_attempt INTEGER NOT NULL DEFAULT 0`

  A fresh database gets them from `create_schema`. **On the user's existing database, the agent drops the old table by hand (T5),** and the app recreates it with the new columns. The app never migrates at startup. The old cost history is thrown away; the user approved that.
- **`TurnTx` knows its turn,** and on a retry, the turn it replaces. When a retry commits, the replaced turn's rows move to the new turn, marked `earlier_attempt = 1`. The reply's cost then **includes what the rejected attempts cost**, and shows that part in a tooltip. When a retry fails, its own rows go to the old turn, marked `earlier_attempt = 1`, so a failed retry's cost shows on the reply that stayed.
- **The image cost row carries the image's asset id and generation time.** The id is created before the request, so the cost row can point at it even if saving the image fails.
- **A See turn's cost** shows on its image's caption, because the See turn has no reply of its own. The caption shows the image's cost and time; its tooltip adds the See turn's total, including the text call that planned the image.
- **Totals in the header don't change.** They still include failed turns and erased exchanges. The per-turn lines show only turns still on the page, so they can add up to less than the header total. That's expected, and the header tooltip says so.

**Why include earlier attempts in the turn cost:** the user asked for "turn cost". A retried reply cost the rejected attempts too, and the header total already counts them. Showing only the final attempt would make a retried turn look cheaper than it was. The tooltip shows the split.

**Not possible:** per-turn costs for turns played before this change. Their rows have no `turn_id`. Those turns show no cost line, and their costs remain in the header total.

## Ground rules

- **Start point:** create `turn-costs` from `main` at `cab909d`, after the startup cleanup. If `main` has moved, branch from the current `main` and record its commit.
  - `git status` must be clean first.
  - The code paths below were checked against `cab909d` (`features/transcript/…`, `TranscriptEntryView.tsx`, `turn/tx.rs`, `images/generation.rs`).
- **No migration code in the app, ever.** The user's rule (2026-09-29):
  - `create_schema` in `shared/db.rs` only creates the current schema on a fresh database.
  - **Don't add** any code that runs at startup to change an existing database: no `ALTER TABLE`, no column or table checks (`pragma_table_info`, `sqlite_master` lookups), no `migration_*` marker rows, and no "upgrade if old" branches.
  - An existing database is changed **by hand,** separately, by the agent with the app stopped (T5).
  - **Check before each commit,** and record it in the commit message: `rg -n "ALTER TABLE|pragma_table_info|table_info|migration" src-tauri/src` has no hits outside `#[cfg(test)]` code.
  - If a fix seems to need a startup migration, write it as a by-hand script instead.
- **Run the whole plan in one go, without asking the user anything.** The user has approved every step, including migrating the real database by hand (T5) and fast-forwarding `main`. Where something is unclear, choose, and record the choice in the report.
- **Fix, don't stop.** When a check fails:
  1. Find the cause and fix it on `turn-costs`, with a unit test when it can be reproduced in one. Commit it as "Fix …", with its line counts and cause.
  2. Rerun the four checks, then the failed check and everything after it.
  3. Up to **5 attempts per check**. If a check still fails, record it as failed with what you tried, carry on, and **don't fast-forward `main`**.
  - Fixes stay within what the check needs. Environment problems (a stuck process, a locked file) get an environment fix.
- **Destructive database changes are allowed.** The user doesn't use the app right now, and its data doesn't matter.
  - The agent may drop tables or delete the `story-llm.sqlite3*` files, in `%APPDATA%\com.story-llm.app\`.
  - **The API keys are the one exception.** They live only in `secrets.json`, not in the database. Never delete, overwrite, open, print or copy `secrets.json`.
  - Never delete git history or `origin`. **Never push.**
- **One commit per code step,** T0 to T4. Each message states its production and test line counts from `git diff --numstat`.
- **After every commit:**
  - `cargo test --manifest-path src-tauri/Cargo.toml`
  - `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` with zero warnings
  - `node --test src/features/story/store.test.mjs src/features/transcript/replacement.test.mjs src/features/usage/store.test.mjs`. Before the T0 commit, the last file is `src/features/stats/store.test.mjs`.
  - `npx tsc --noEmit`
- **The report** goes in this repository at `docs/report/turn-costs.md`.
  - Create `docs/report/` if it's missing.
  - `docs/` is git-ignored, so the report is never committed.
  - It links each evidence file by its full path.
- **Evidence files** go in `%TEMP%\codex\turn-costs\`. Every QA check saves its output to a file. A check without a saved file is "no evidence", never a pass.
- **QA drives the app like a user,** with real clicks and typing through tauri-pilot. Database reads are read-only `SELECT`s. Never edit the DOM, app state or rows to make a check pass.
- **Model spending cap:** at most **10 text model calls and 3 images** across QA, including fix reruns.
- **tauri-pilot notes from earlier runs:**
  - Start the app with the `run` skill if it's available, otherwise `pnpm tauri dev`.
  - The pipe is `\\.\pipe\tauri-pilot-com.story-llm.app`. Drive it from **PowerShell**; Git Bash mangles the pipe path.
  - PowerShell 5.1 splits double-quoted arguments that contain spaces. Use here-strings, or prefix selectors like `[aria-label^=New]`.

---

## T0. Rename `stats` to `usage`, and the header component (names only)

**Backend:**
- `src-tauri/src/features/stats/` → `src-tauri/src/features/usage/`, and `features/mod.rs`: `pub mod usage;`.
- Update every `use crate::features::stats::…` → `usage::…`. Today these are in `images/generation.rs`, `narrator/stream.rs`, `stories/auto_title.rs` and `turn/tx.rs`, where the alias `stats_repository` becomes `usage_repository`. Also update `lib.rs`.
- Rename the command `get_story_stats` → `get_story_usage` (the function and its `lib.rs` entry).
- `StoryStats` → `StoryUsage`.

**Frontend:**
- `src/features/stats/` → `src/features/usage/`
- `StatsBar.tsx` / `StatsBar` → `StoryHeader.tsx` / `StoryHeader`
- `useStatsStore` → `useUsageStore`
- `statsApi` → `usageApi`
- the `StoryStats` type → `StoryUsage`
- the invoke name → `"get_story_usage"`
- the import in `transcript/StoryView.tsx`
- `src/features/stats/store.test.mjs` → `src/features/usage/store.test.mjs`, with its mock command name updated

**Leave these unchanged:** the table `usage_records`, `UsageKind`, `UsageRecord`, `CallUsage`, and the word "stats" in the Narrator Tools descriptions (those mean character stats).

**Check:** `rg -n "[Ss]tats" src src-tauri/src` afterwards. Only the two Narrator Tools descriptions remain.

**Line estimate:** net about 0. State the lines changed.
**Commit:** "Rename the stats module to usage and the stats bar to StoryHeader".

From T0 on, the frontend test list in the ground rules uses `src/features/usage/store.test.mjs` instead of `src/features/stats/store.test.mjs`.

---

## T1. Record which turn and which image each cost belongs to (backend)

**`src-tauri/src/shared/db.rs`,** in `CREATE TABLE IF NOT EXISTS usage_records`, add the columns **at the end, after `created_at TEXT NOT NULL`**:
```sql
turn_id TEXT,
image_asset_id TEXT,
duration_ms INTEGER,
earlier_attempt INTEGER NOT NULL DEFAULT 0
```
and add `CREATE INDEX IF NOT EXISTS idx_usage_turn ON usage_records(turn_id);` after `idx_usage_story`.
- **Why at the end:** a future by-hand `ALTER TABLE … ADD COLUMN` can only append, so new columns go last as a habit. This plan doesn't use `ALTER`; T5 drops the table.
- **No migration code,** following the ground rules. `create_schema` gains only these lines.
- `fresh_database_has_transcript_table` in `db.rs` gains one assertion: the last four columns of `usage_records`, in order, are `turn_id`, `image_asset_id`, `duration_ms` and `earlier_attempt`.
- **`README.md`, "Local data":** change "after a schema change, delete the `story-llm.sqlite3*` files" to "after a schema change, change the database by hand (for example, drop the changed table) or delete the `story-llm.sqlite3*` files". Keep the rest of the sentence. This is about +1 line.

**`src-tauri/src/features/usage/model.rs`:**
- `UsageRecord` gains `pub turn_id: Option<String>`, `pub image_asset_id: Option<String>` and `pub duration_ms: Option<u64>`.
- `UsageRecord::text(...)` leaves all three `None`.
- `UsageRecord::image(model, cost_usd)` becomes `UsageRecord::image(model, cost_usd, asset_id: Option<String>, duration_ms: Option<u64>)`.

**`src-tauri/src/features/usage/repository.rs`:**
- `insert` writes the three new columns, plus `earlier_attempt` = 0.
- `record_late(pool, story_id, record)` is unchanged. The caller now puts `turn_id` on the record.

**`src-tauri/src/features/turn/tx.rs`:**
- **New fields:**
  - `turn_id: StdMutex<Option<String>>`
  - `replaces_turn: StdMutex<Option<String>>`
- **New methods:**
  - `pub fn set_turn(&self, turn_id: &str)`
  - `pub fn replaces(&self, old_turn_id: &str)`
  - `pub fn turn_id(&self) -> Option<String>`
- **`flush_usage` gains three arguments.** It's a free function today, `flush_usage(story_id, usage, conn)` at `tx.rs:22`, so that `Drop` can pass `self.conn.get_mut()`.
  - The new arguments: `turn_id: Option<&str>`, `replaces: Option<&str>` and `committed: bool`.
  - Each caller reads the two ids from their mutexes first, then calls it: `commit()` with `true`, `rollback()` and `Drop` with `false`.
  - Keep the call order as it is. In `commit()`, `flush_usage` runs **before** `COMMIT`; in `rollback()` and `Drop`, it runs **after** `ROLLBACK`.
  - **On commit:** for each record without a `turn_id`, use the current `turn_id`, then insert. Then, if `replaces_turn` is set, call `usage::repository::move_to_turn(conn, story_id, old, new)`, a new function in the usage module:
    ```sql
    UPDATE usage_records SET turn_id = ?new, earlier_attempt = 1 WHERE story_id = ?story AND turn_id = ?old
    ```
    This runs inside the transaction, before `COMMIT`. `tx.rs` holds no SQL for `usage_records` (see the ownership rule).
  - **On rollback or drop:** if `replaces_turn` is set, every record gets `turn_id = old` and `earlier_attempt = 1`. Otherwise records keep the current `turn_id`, a turn that no longer exists: that cost only counts in the header total.
  - `insert` needs to write `earlier_attempt`. Add the flag to `insert` as a parameter; don't add it to `UsageRecord`.

**Callers:**
- **`turn/submit.rs`:** right after `turns::create_turn(conn, &story_id)?` in `prepare_and_spawn`, call `turn.set_turn(&turn_id)`. That call happens inside a `turn.with` closure, which borrows only `conn`, so capture `turn` by reference or set it right after the closure returns.
- **`turn/retry.rs`:** in `prepare_retry`, after finding `old_turn`, call `turn.replaces(&old_turn.id)`.
- **`stories/auto_title.rs`:** the late-title path puts `turn.turn_id()` on each late record.

**`src-tauri/src/features/images/generation.rs`:**
1. **Create the asset id first:** `let asset_id = Uuid::new_v4().to_string();` before the request. `persist_and_store_image` takes the id instead of making one. Check that `images::commands` and the tests still build.
2. **Time the request:** start `std::time::Instant::now()` just before spawning it, and take `elapsed()` when it finishes.
3. **Finished in time:** `turn.record_usage(UsageRecord::image(&model, cost, Some(asset_id.clone()), Some(ms)))`, before `with_savepoint` persists the image, as today.
4. **Late path:** the late record carries `turn_id: Some(target.turn_id.clone())`, and `image_asset_id: None`, because the image is discarded.
   - If the request finishes, `duration_ms` is the total time until then.
   - If `LATE_REPLY_LIMIT` passes, `duration_ms` is `None`.

**Tests:**
- **`turn/tx.rs`:**
  1. **`usage_gets_the_turn_id_on_commit`:** `set_turn("t1")`, record twice, commit. Both rows have `turn_id = 't1'` and `earlier_attempt = 0`.
  2. **`retry_commit_moves_old_rows_to_the_new_turn`:** rows for `t1` exist. A new `TurnTx` with `replaces("t1")` and `set_turn("t2")` records one, then commits. All three rows have `turn_id = 't2'`; the two old ones have `earlier_attempt = 1` and the new one 0.
  3. **`failed_retry_rows_go_to_the_old_turn`:** the same setup, then `rollback()`. The new row has `turn_id = 't1'` and `earlier_attempt = 1`, and the old rows are untouched.
  4. **`drop_after_retry_behaves_like_rollback`:** the same as test 3, but via drop.
- **`usage/repository.rs`:**
  - `insert` round-trips `turn_id`, `image_asset_id` and `duration_ms`.
  - **`move_to_turn_touches_only_that_story_and_turn`:** rows for `(s, t1)`, `(s, t9)` and `(other, t1)`. After `move_to_turn(conn, "s", "t1", "t2")`, only the `(s, t1)` rows are now `t2` with `earlier_attempt = 1`.
- **Ownership check** (in the commit message): `rg -n "usage_records" src-tauri/src --glob '!**/usage/**' --glob '!**/shared/db.rs'` shows no hits outside tests.

**Line estimate:** production +55 to +85, tests +80 to +120.
**Commit:** "Record the turn, image and generation time for each cost".

---

## T2. A command for per-turn and per-image costs (backend)

**`src-tauri/src/features/usage/model.rs`:**
```rust
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct TurnCost {
    pub turn_id: String,
    pub text_cost_usd: f64,
    pub image_cost_usd: f64,
    pub total_cost_usd: f64,
    pub earlier_attempts_cost_usd: f64,
    pub unpriced_calls: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImageCost {
    pub asset_id: String,
    pub turn_id: Option<String>,
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct StoryCostBreakdown {
    pub turns: Vec<TurnCost>,
    pub images: Vec<ImageCost>,
}
```

**`src-tauri/src/features/usage/repository.rs`:** `pub fn cost_breakdown(conn, story_id) -> AppResult<StoryCostBreakdown>`, as two queries.
- **Turns:**
  ```sql
  SELECT turn_id,
    COALESCE(SUM(CASE WHEN kind <> 'image' THEN cost_usd END), 0),
    COALESCE(SUM(CASE WHEN kind = 'image' THEN cost_usd END), 0),
    COALESCE(SUM(CASE WHEN earlier_attempt = 1 THEN cost_usd END), 0),
    COUNT(CASE WHEN cost_usd IS NULL THEN 1 END)
  FROM usage_records WHERE story_id = ?1 AND turn_id IS NOT NULL
  GROUP BY turn_id
  ```
  `total_cost_usd` = text + image, computed in Rust.
- **Images:**
  ```sql
  SELECT image_asset_id, turn_id, cost_usd, duration_ms FROM usage_records
  WHERE story_id = ?1 AND image_asset_id IS NOT NULL
  ```

**`src-tauri/src/features/usage/commands.rs`:** `#[tauri::command] pub fn get_story_usage_breakdown(pool: State<Pool>, story_id: String) -> AppResult<StoryCostBreakdown>`. It's a plain `fn`, like `get_story_usage`. Register it in `lib.rs` after `get_story_usage`.

**Tests** in `usage/repository.rs`:
1. **Setup:**
   - turn `t1`: narration $0.004 and an image $0.039 (asset `a1`, 14,200 ms)
   - turn `t2`: narration $0.003, plus an earlier-attempt narration of $0.002, plus a title with no cost
   - a row with no turn: $0.010
2. **Expect:**
   - `t1`: text 0.004, image 0.039, total 0.043, earlier 0, unpriced 0
   - `t2`: text 0.005, image 0, total 0.005, earlier 0.002, unpriced 1
   - no entry for the row without a turn
   - `images` = `[a1, t1, 0.039, 14200]`
   - another story's rows don't appear

**Line estimate:** production +45 to +65, tests +40 to +60.
**Commit:** "Add a per-turn and per-image cost breakdown command".

---

## T3. Frontend: store and cost lines

**`src/shared/types.ts`:** `TurnCost`, `ImageCost` and `StoryCostBreakdown`, matching T2 in snake_case.

**`src/features/usage/api.ts`:** `breakdown: (storyId) => invoke<StoryCostBreakdown>("get_story_usage_breakdown", { storyId })`.

**`src/features/usage/store.ts`:**
- Add `breakdownByStory: Record<string, { turns: Record<string, TurnCost>; images: Record<string, ImageCost> } | undefined>`.
- `load(storyId)` fetches the totals and the breakdown together (`Promise.all`), under the same request-version guard, and stores both. The header keeps calling `load`, so the breakdown reloads whenever a turn ends.
- Add `formatSeconds(ms)`: `"14.2 s"`, one decimal, and `"—"` for null.

**`src/features/transcript/TranscriptEntryView.tsx`:**
1. **The turn cost line.** For a `narration` entry whose `turn_id` has a `TurnCost`, show one line in the existing footer row.
   - Change that row from `justify-end` to `justify-between`, with the cost line on the left and always visible. The action buttons keep their hover-only opacity.
   - Style: `text-[11px] text-muted tabular-nums`.
   - Text: `Turn $0.0123 · text $0.0040 · images $0.0083`. When `image_cost_usd` is 0: `Turn $0.0040`.
   - Append `*` when `unpriced_calls > 0`.
   - **Tooltip** (`title`):
     - "Text and images for this turn."
     - When `earlier_attempts_cost_usd > 0`: "Includes $0.0020 from earlier attempts (Retry)."
     - When unpriced: "N calls reported no cost."
   - **Hide the line** while this entry is being replaced (`isBeingReplaced`) or streaming.
2. **The image caption line.** For each rendered image with an `ImageCost`, add a line under the image, next to `ImageCaption`: `$0.0390 · 14.2 s`.
   - A missing cost shows `cost unknown`; a missing duration shows no time.
   - **Tooltip:** "Image cost and generation time." When the image's `turn_id` differs from the entry's `turn_id` (a See turn), also: "See turn total $0.0401, including $0.0011 to plan the image", using that turn's `TurnCost`.
3. `formatUsd` comes from `usage/store.ts`, as in the header.

**`src/features/usage/StoryHeader.tsx`:** add to the Total tooltip: "Per-turn lines in the story show only turns still on the page; failed and erased turns count here too."

**Tests** in `src/features/usage/store.test.mjs`:
1. `load` stores the breakdown keyed by turn and by asset.
2. The stale-response rule covers the breakdown too: an older `load` doesn't overwrite a newer one.
3. `formatSeconds`: 14200 → "14.2 s"; null → "—".

**Line estimate:** production +60 to +90, tests +25 to +40.
**Commit:** "Show each turn's cost and each image's cost and time in the transcript".

---

## T4. A live timer while an image generates

**`src/features/transcript/ImagePlaceholder.tsx`:**
- On mount, remember `Date.now()`.
- Update every 100 ms with `setInterval`, and clear it on unmount.
- Show `Illustrating this scene... 7.3 s` with one decimal, in the existing label.
- Add `aria-live="off"`, so screen readers don't announce every tick.
- The placeholder mounts when `scene-image-pending` arrives and unmounts when the image arrives or fails. The timer therefore covers the generation time the user sees. The final, exact time comes from T3's caption line (backend `duration_ms`).
- **During a Retry of a turn with an image,** the placeholder remounts, and the timer starts again. That's correct: it's a new request.

**No unit test.** It's covered in the T6 QA (check 3).

**Line estimate:** production +10 to +15.
**Commit:** "Show a live timer while a scene image generates".

---

## T5. Drop the old usage table by hand

The user's `usage_records` table lacks the new columns, and `CREATE TABLE IF NOT EXISTS` won't add them. The app has no migration code, so the agent drops the table once, by hand, and the app recreates it with the new columns at the next start. The old cost history is thrown away, as the user approved. The `settings` table, with the text and image model choices, is kept. No commit, since nothing in the repository changes.

1. Stop every `story-llm.exe`, and every `tauri dev` / `vite` started from this repository (`t5-processes.txt`).
2. **If `%APPDATA%\com.story-llm.app\story-llm.sqlite3` doesn't exist,** record that and stop here. The app creates a fresh database in T6.
3. With Python's `sqlite3`, run `DROP TABLE IF EXISTS usage_records;`. Its indexes go with it.
4. **Check,** and save as `t5-drop.txt`:
   - `SELECT type, name FROM sqlite_master WHERE name LIKE '%usage%'`, before (the table and `idx_usage_story`) and after (nothing)
   - `PRAGMA integrity_check` returns `ok`
   - `PRAGMA foreign_key_check` returns no rows
5. **If anything goes wrong,** stop the app, delete the `story-llm.sqlite3*` files, and let T6 start fresh. That's allowed. Never touch `secrets.json`.

---

## T6. QA with tauri-pilot on the real app

Start the `turn-costs` build with the normal identifier, `com.story-llm.app`, with tauri-pilot (see the notes in the ground rules). Save every probe.

**Setup:**
- Check that the Image Model panel is enabled and has a key. Record booleans only.
- If image generation is disabled, enable it in the panel. That's a real click, and it's allowed.
- Click "+ New story". Keep all narrator tools on.

**Checks:**
1. **Schema:**
   - `usage_records` has `turn_id`, `image_asset_id`, `duration_ms` and `earlier_attempt`, and `idx_usage_turn` exists.
   - There are no `migration%` keys in `settings`.
   - `usage_records` starts with 0 rows, recreated by `create_schema` after T5's drop.
   - The app log shows no schema errors.
   - Save as `t6-01-schema.json`.
2. **A text turn:**
   - Submit "Do: Look around the room." and wait until it's done.
   - **Pass if:**
     - the reply shows `Turn $…`
     - its value equals the sum of `usage_records.cost_usd` for that turn's `turn_id` (read the turn id from `list_transcript_entries`)
     - the header Total is at least that value
   - Save as `t6-02-text-turn.json`.
3. **A See turn, with the timer:**
   - Select See and submit.
   - While the placeholder shows, read its label twice, at least 1 s apart. **Pass if** the second number is larger.
   - When the image arrives:
     - the caption line shows `$… · … s`
     - its cost equals that image's `usage_records` row, found by `image_asset_id` = the image's id
     - its time equals `duration_ms` / 1000, to one decimal
     - its tooltip contains "See turn total"
   - Save as `t6-03-see.json`.
4. **Retry includes earlier attempts:**
   - Submit "Do: Open the door.", wait, and note its turn cost.
   - Click Retry and wait.
   - **Pass if:**
     - the new reply's turn cost is **greater** than the new attempt's own rows alone
     - it equals the sum of all rows now on that turn id
     - its tooltip says "Includes $… from earlier attempts"
     - the header Total rose
   - Save as `t6-04-retry.json`.
5. **Restart:**
   - Stop and start the app, then open the same story.
   - **Pass if:** every turn line and image caption shows the same values as before (`t6-05-restart.json`).
6. **Totals are consistent:**
   - Read the header Total and the sum of all shown turn lines.
   - **Pass if:**
     - the header Total is greater than or equal to the sum of the lines
     - the header Total equals `get_story_usage.total_cost_usd`
   - Save as `t6-06-totals.json`.
7. **Erase:**
   - Erase the last exchange.
   - **Pass if:** its turn line disappears with it, and the header Total **doesn't drop**, because the money was spent (`t6-07-erase.json`).
8. **Logs:** no unexpected `ERROR` or `panicked` in the app log for this run (`t6-08-logs.txt`).

Stop the app you started. Leave the test story in place; the user may look at it.

---

## T7. Fast-forward `main`

- **Only if every T6 check passed** (after fixes):
  - `git checkout main`
  - `git merge --ff-only turn-costs`
  - save `git log --oneline -1` as `t7-main.txt`
  - **Don't push.**
- **If `main` moved:** merge `main` into `turn-costs`, rerun the four checks and T6 checks 2 and 5, then fast-forward.
- **If a T6 check failed after 5 attempts:** leave `main` as it is, and name the failed checks.
- Leave `main` checked out with a clean tree.

**Final message and the report, `docs/report/turn-costs.md`** (the only things the user reads):
- whether `main` moved, and to which commit
- each T6 check with its result and file
- every "Fix …" commit with its cause
- the model calls and images used, and their cost
- T5: whether `usage_records` was dropped, or the database was missing, and that `secrets.json` was untouched
- the no-migration-code check for each commit
- every commit's line counts
- anything done that the plan didn't name

---

## Line count audit

| Step | Production | Tests |
|---|---|---|
| T0 rename stats → usage, StatsBar → StoryHeader | about 0 (renames) | about 0 |
| T1 turn, image and time on each cost | +55 to +85, plus README +1 | +80 to +120 |
| T2 breakdown command | +45 to +65 | +40 to +60 |
| T3 store and cost lines | +60 to +90 | +25 to +40 |
| T4 image timer | +10 to +15 | 0 |
| **Total** | **+170 to +255** (Rust +100 to +150, frontend +70 to +105) | **+145 to +220** |

**Where the lines could be fewer:** showing only the **text** cost per turn would skip the image join and the caption line, saving about 40 production lines. The user wanted both, and the image data is needed for the timer anyway.

## Not in this plan

- **Refreshing a timed-out image's late cost on screen as soon as it arrives.** It's written after the turn ends, and appears at the next usage reload (the next turn, or reopening the story). A push event would need an `AppHandle` in `record_late`.
- **A cost chart over time.**
