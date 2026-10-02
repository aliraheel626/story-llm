# Image captions: a vision model describes each generated image for the narrator

## Context

When a scene image is generated, the narrator can learn about it in two ways today:

| Toggle (per story, Context → Transcript) | What the narrator gets | Default |
|---|---|---|
| **Image prompts** (`record.image_generated`) | "A scene image was generated depicting: {description}", i.e. what was **asked for** | off |
| **The images themselves** (`images`) | the picture, up to 4 since the last summary | off |

The prompt is written *before* the picture exists, so it can't describe what the image model actually drew: invented details, a changed outfit, an extra figure. Sending the picture itself costs about 1,000–1,500 tokens and about 1 MB per request, and needs a narrator that can read images.

**The user chose a third option:**
- After each image is saved, a **user-selectable caption model** (a cheap vision model on OpenRouter) describes it in 2–3 sentences.
- The caption is saved as its own record, `image_captioned`, with its own toggle, **Image captions**, next to Image prompts.
- The three stay independent: See (player action), Image prompts, Image captions, and The images themselves.

**Where it runs:** image generation already happens inside the turn transaction (`images::generate_in_turn`, called from `turn/submit.rs:215` for See and `:258` otherwise), before `turn.commit()`. Captioning runs right after each image is saved, still inside the turn. So:
- the caption commits or rolls back with its turn
- Retry and Erase clean it up like the image
- it's in the `StoryImage` emitted with `scene-image-generated`
- it adds about 2–4 s to a turn that makes an image

**Decisions already made (don't revisit):**
- **The caption model is user-selectable,** in the Image Model panel. It's stored in the same `image_model_default` settings row as the image model.
- **Default caption model:** `google/gemini-3.5-flash-lite`. On OpenRouter it takes text, image, video, file and audio, outputs text, and costs $0.30/M in and $2.50/M out, so about **$0.0006 per caption**.
- **Three separate controls:**

  | Control | Where | What it decides | Default |
  |---|---|---|---|
  | **"Caption images"** checkbox | Image Model panel → **Captions** section (global) | whether a caption is **made** for each image | on |
  | **"Caption model"** picker | the same **Captions** section, directly under the checkbox | which vision model makes it. No "Off" choice; an empty or missing value reads as the default model. | Gemini 3.5 Flash Lite |
  | **"Image captions"** toggle | Context → Transcript → Records (per story) | whether the narrator is **sent** the captions, like every other Records toggle | on |

- **A caption is made when image generation and "Caption images" are both on.** The per-story toggle never decides this. When a caption is made but the story's toggle is off, the caption still shows under the image in the UI; it just isn't sent to the narrator.
- **When "Caption images" is off,** the model picker stays visible but is disabled, and keeps its value.
- **Toggle defaults:** Image captions **on**; Image prompts stays **off**; The images themselves stays **off**.
- **A caption failure never fails the image:**
  - A timeout, HTTP error or empty reply means no caption record, and a warning in the log.
  - The image and its `image_generated` record are unaffected.
- **Caption cost** is a new usage kind, `caption`. It counts toward a turn's and the story's **image** cost, but **not** toward `image_count`, and not toward the per-image cost shown under each picture.

## Ground rules

- **Start point:** a new branch `image-captions` from `main` at `9f6117d`, with `git status` clean. `docs/` is git-ignored. Don't rewrite existing commits.
- **Run the whole plan in one go, without asking the user anything.** Where something is unclear, choose, and record the choice in the report.
- **Fix, don't stop.** When a check fails:
  1. Find the cause and fix it on `image-captions`, with a unit test when it can be reproduced in one. Commit it as "Fix …", with its line counts and cause.
  2. Rerun the checks below, then the failed check and everything after it.
  3. Up to **5 attempts per check**, within the spending cap. If a check still fails, record it with what you tried, carry on, and **don't merge into `main`**.
- **No migration code in the app.** The one schema change (the `usage_records` CHECK list) goes into the `CREATE TABLE` in `shared/db.rs` for new databases, and is applied to the existing database **by hand** in step C0. Before each commit, `rg -n "ALTER TABLE|pragma_table_info|table_info|migration" src-tauri/src` must have hits only in `#[cfg(test)]` code.
- **Database and secrets:**
  - The database is `%APPDATA%\com.story-llm.app\story-llm.sqlite3`.
  - Destructive database changes are allowed.
  - **Never** open, print, copy, overwrite or delete `secrets.json`, which sits in the same folder. Any copy or backup of that folder must name the `.sqlite3*` files explicitly, never the whole folder.
  - **Never push.**
- **One commit per step** (C1, C2, C3, plus any "Fix …"). Each message states its production and test line counts from `git diff --numstat`.
- **After every commit:**
  - `cargo test --manifest-path src-tauri/Cargo.toml`
  - `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` with zero warnings
  - `node --test src/features/story/store.test.mjs src/features/transcript/replacement.test.mjs src/features/usage/store.test.mjs`
  - `npx tsc --noEmit`
- **The report** goes to `docs/report/image-captions.md`, and it links each evidence file by its full path.
- **Evidence files** go in `docs/report/image-captions/`, next to the report. Create the folder; `docs/` is git-ignored. Each QA check saves what it actually saw there: DOM captures, read-only query results, log excerpts. A check without a saved file is "no evidence", never a pass.
- **QA drives the app like a user,** with real clicks and typing through tauri-pilot. Database reads are read-only (`mode=ro`). Never edit the DOM, app state or rows to make a check pass.
- **Spending cap:** at most **15 text model calls, 4 images and 6 captions** in total, including fix reruns. That's about $0.40.
- **tauri-pilot notes:**
  - Start the app with `pnpm tauri dev`.
  - The pipe is `\\.\pipe\tauri-pilot-com.story-llm.app`. Drive it from **PowerShell**; Git Bash mangles the pipe path.
  - PowerShell 5.1 splits double-quoted arguments that contain spaces. Use here-strings, or prefix selectors like `[aria-label^=New]`.
  - Poll with read-only DOM captures in a loop (every 300–500 ms) rather than waiting and then capturing once.

---

## C0. Widen the usage CHECK in the existing database (by hand, no commit)

SQLite can't change a CHECK constraint in place, so rebuild `usage_records` once. The rebuild keeps every row.

1. Close the app.
2. Copy the database files `story-llm.sqlite3`, `story-llm.sqlite3-wal` and `story-llm.sqlite3-shm` (if present), **by name**, to `docs/report/image-captions/db-backup/`.
3. Run this once against `story-llm.sqlite3` with Python's `sqlite3` module. It's a one-off script in `docs/report/image-captions/`, never app code.
   ```sql
   PRAGMA foreign_keys = OFF;
   BEGIN;
   ALTER TABLE usage_records RENAME TO usage_records_old;
   CREATE TABLE usage_records ( /* copy the exact column list from shared/db.rs */
       ... kind TEXT NOT NULL CHECK (kind IN ('narration', 'summary', 'title', 'image', 'caption')), ...
   );
   INSERT INTO usage_records SELECT * FROM usage_records_old;
   DROP TABLE usage_records_old;
   CREATE INDEX IF NOT EXISTS idx_usage_story ON usage_records(story_id);
   CREATE INDEX IF NOT EXISTS idx_usage_turn ON usage_records(turn_id);
   COMMIT;
   PRAGMA foreign_keys = ON;
   ```
4. **Check,** read-only:
   - the row count before and after is equal
   - `SELECT sql FROM sqlite_master WHERE name = 'usage_records'` contains `'caption'`
   - `PRAGMA integrity_check` returns `ok`

   Save all three as `c0-schema.txt`.

---

## C1. Caption model setting

**`src-tauri/src/features/images/openrouter.rs`:** add `pub const DEFAULT_CAPTION_MODEL: &str = "google/gemini-3.5-flash-lite";` next to `DEFAULT_IMAGE_MODEL`. **+1.**

**`src-tauri/src/features/settings/model.rs`:** add to `ImageModelSettings`:
- `pub captions_enabled: bool`, with the doc comment "Whether each generated image gets a caption."
- `pub caption_model: String`, with the doc comment "Vision model that writes the captions."

**+4.**

**`src-tauri/src/features/settings/repository.rs`:**
- **`read_image_model_settings`:**
  - Read `v.get("caption_model").and_then(|m| m.as_str()).map(str::trim).filter(|m| !m.is_empty()).unwrap_or(DEFAULT_CAPTION_MODEL)`.
  - A missing key and an empty value both mean the default.
  - Read `v.get("captions_enabled").and_then(|e| e.as_bool()).unwrap_or(true)`.
  - The no-row branch uses both defaults (`true` and `DEFAULT_CAPTION_MODEL`).
  - Add both to the returned struct. **+8.**
- **`write_image_model_settings`:**
  - Add `captions_enabled: bool` and `caption_model: String` parameters.
  - Trim `caption_model`; if it's empty, store `DEFAULT_CAPTION_MODEL`, as `style` already does with its default.
  - Add `"captions_enabled"` and `"caption_model"` to the stored JSON.

  **+4.**
- **Test:**
  - a row without either key reads as captions on, with the default model
  - a written `""` model reads back as the default
  - a written model id and `captions_enabled: false` read back unchanged

  **+15 to +20 test lines.**

**`src-tauri/src/features/settings/text_model.rs`:** add
```rust
/// `Some(false)` only when OpenRouter's listing says the model can't take images.
pub(super) async fn openrouter_accepts_images(model: &str) -> Option<bool> {
    fetch_model("https://openrouter.ai/api/v1/models", ("data", "id"), model, None)
        .await
        .ok()
        .as_ref()
        .and_then(openrouter_image_support)
}
```
**+8.**

**`src-tauri/src/features/settings/commands.rs` → `save_image_model_settings`:**
- Add `captions_enabled: bool` and `caption_model: String` arguments.
- **Before** the `blocking(...)` write, and only when `captions_enabled` is true:
  - if `openrouter_accepts_images(&caption_model).await == Some(false)`, return `AppError::Invalid(format!("{caption_model} can't read images; pick a vision model for captions"))`
  - `None` (the lookup failed, or the id isn't listed) saves anyway, with a `log::warn!`
- Pass the value into `write_image_model_settings`.

**+8 to +10.**

**Frontend:**
- **`src/shared/types.ts`:** `ImageModelSettings` gains `captions_enabled: boolean; caption_model: string`. **+0/−0** (the same line gets longer).
- **`src/features/settings/api.ts`:** `imageModelApi.save(model, enabled, style, captionsEnabled, captionModel)` sends both. **+0 to +1.**
- **`src/features/settings/ImageModelPanel.tsx`:**
  - Add a `CAPTION_MODELS` list, with the same shape as `IMAGE_MODELS`. There's **no "Off" entry.**
    - `google/gemini-3.5-flash-lite`: "Gemini 3.5 Flash Lite (default)"
    - `google/gemini-3.8-flash`: "Gemini 3.8 Flash (sharper)"
    - `google/gemini-2.5-flash-lite`: "Gemini 2.5 Flash Lite (cheapest)"
    - `openai/gpt-5-nano`: "GPT-5 nano"

    All four ids were checked on OpenRouter on 2026-09-30: image input, text output.
  - After the Style field, add **one "Captions" section** holding all the caption controls together: a small heading "Captions", styled like the existing field labels, then these three items in order, nothing else in between.
  - First, a **"Caption images"** checkbox, styled like "Enable image generation". It's disabled when image generation is off.
  - Directly under it, a "Caption model" `<select>` with a "Custom…" text input, mirroring the existing Model select. It's disabled when image generation **or** "Caption images" is off.
  - Last, a muted line: "Describes each image in words, shown under the image. About $0.0006 per image. Whether the narrator sees captions is set per story under Context → Transcript."
  - State: `captionsEnabled`, `captionModel` and `captionCustom`, loaded in the `useSettingsForm` callback. Pass `captionsEnabled` and `captionModel.trim()` to `save`. The Save button is also disabled when captions are on and `captionModel.trim()` is empty.
  - Show a save error through the store's existing error path. Check how `useSettingsForm` surfaces errors, and don't add a new one.

  **+36 to +46.**

**Commit C1:** "Add caption settings to the image settings". **Production about +66 to +80, tests +15 to +20.**

---

## C2. Generate, store and send captions

### Record kind and toggle
- **`src-tauri/src/features/transcript/model.rs`:** add `pub const IMAGE_CAPTIONED: &str = "image_captioned";`, and add it to `RECORD_KINDS` **right after** `IMAGE_GENERATED`. **+2.**
- **`src-tauri/src/features/transcript/filter.rs` → `record_label`:** add `kind::IMAGE_CAPTIONED => ("Image captions", true),`. **+1.**
- **Sending it to the narrator needs no change.** The generic record path in `transcript/history.rs:194-218` renders it as `[Authoritative story event: image_captioned]\n{content}` whenever `record.image_captioned` is on.
- **Test** in `history.rs`, next to the test at about line 733: an `image_captioned` entry is included only when its toggle is on, with its content. **+15 test lines.**
- The existing test `catalog_resolves_defaults_and_rejects_unknown_saves` (`stories/settings.rs:408`) derives record keys from `RECORD_KINDS`, so it passes unchanged. Add `assert!(defaults.includes("record.image_captioned"));`. **+1 test line.**

### The vision call
**`src-tauri/src/ai/mod.rs`:** add
```rust
/// One vision call that describes an image in words.
pub async fn describe_image(
    config: &TextModelConfig,
    prompt: &str,
    media_type: &str,
    bytes: &[u8],
) -> (AppResult<String>, Vec<CallUsage>)
```
- Build the agent with `build_agent(config, "", Vec::new(), None)`.
- The prompt is one `Message::User` with `UserContent::text(prompt)` and `UserContent::image_base64(STANDARD.encode(bytes), Some(media_type), None)`, where the media type is resolved with `ImageMediaType::from_mime_type`, exactly as `history_message` does (lines 161-176). An unknown MIME type returns `AppError::Invalid`.
- Send it with `agent.prompt(message).add_hook(UsageHook { sink })`, collecting usage like `prompt_typed` (lines 542-560).
- If Rig's `prompt` doesn't take a `Message` in this version, use the lowest-level Rig completion call that does. Keep the usage hook, and record which call you used in the report.

**+25 to +30.**

**`src-tauri/src/prompts.rs`:** add
```rust
pub fn caption_prompt(character_names: &[&str]) -> String
```
Its text:
> Describe this story illustration in 2–3 plain sentences for a narrator who cannot see it. State only what is visible: who and what is in the frame, clothing, expressions, positions, the setting, lighting and notable objects. Don't interpret the story, and don't mention the art style.

When `character_names` isn't empty, append:
> Characters who may appear: {names joined with ", "}. Use a name only when a figure clearly matches; otherwise describe the figure.

**+12.** **Test:** the names sentence is present only when names are given. **+8 test lines.**

### Running it after each image
**`src-tauri/src/features/usage/model.rs`:**
- Add `UsageKind::Caption`, with `as_str` returning `"caption"`.
- Add `UsageRecord::caption(model: &str, usage: CallUsage) -> Self`, with provider `"openrouter"`, no turn and no asset.

**+14.**

**`src-tauri/src/shared/db.rs`:** add `'caption'` to the `usage_records.kind` CHECK list. This is for new databases; C0 did the existing one. **+0.**

**`src-tauri/src/features/usage/repository.rs`:**
- In `story_usage` and `cost_breakdown`, change every `kind <> 'image'` to `kind NOT IN ('image', 'caption')`, and every `SUM(CASE WHEN kind = 'image' …)` to `kind IN ('image', 'caption')`.
- **Leave** `COUNT(CASE WHEN kind = 'image' …)`, which is `image_count`, **and** the per-image query (`image_asset_id IS NOT NULL`) unchanged. Caption rows carry no `image_asset_id`.

**+0** (4 lines edited). **Test:** a caption row adds to `image_cost_usd` and `total_cost_usd`, not to `text_cost_usd` or `image_count`. **+12 test lines.**

**`src-tauri/src/features/images/generation.rs`:**
- **In `generate_from_description`:**
  - Before `persist_and_store_image` consumes `generated`, keep `let (media_type, bytes) = (generated.media_type.clone(), generated.bytes.clone());`.
  - After the savepoint returns the image, make it `let mut image = …;`.
  - When `settings.captions_enabled`, call `caption_image(...)`. **Don't** read the story's transcript toggle here: it only filters what's sent to the narrator (`history.rs`), never what's generated.
  - Return `Ok(image)`.

  **+7 to +9.**
- **New `async fn caption_image(turn: &TurnTx, model: &str, api_key: &str, target: &ImageTarget, image: &mut StoryImage, names: &[&str], media_type: &str, bytes: Vec<u8>)`:**
  - **Config:** build a `TextModelConfig { provider: "openrouter".into(), model: model.into(), api_key: api_key.into(), context_window: 0, supports_images: true }`.
  - **Request:** run `ai::describe_image` in `tauri::async_runtime::spawn`, under `timeout(CAPTION_TIMEOUT, …)`, with `const CAPTION_TIMEOUT: Duration = Duration::from_secs(30);`.
  - **Timeout:** record one unpriced `UsageRecord::caption(model, CallUsage::default())` through `turn.record_usage`, log a warning, and return.
  - **Otherwise:** record every returned `CallUsage` as `UsageRecord::caption`.
    - An `Err` result, or a text empty after trimming: log a warning and return.
  - **Caption text:** trim it and cap it at 1,000 characters.
  - **Save:** `turn.with_savepoint(|conn| persist_caption(conn, target, &image.id, model, &caption)).await`. On error, log a warning and return.
  - **On success:** set `image.caption = Some(caption)`.
  - The names are the names from the `characters` already looked up for the prompt (`characters_by_ids`).
  - It returns nothing. No error leaves this function.

  **+40 to +45.**
- **New `fn persist_caption(conn, target, asset_id, model, caption) -> AppResult<()>`:**
  - Look up the base entry for `story_id`, as `persist_image_record` does.
  - `append_entry(conn, &story_id, transcript_kind::IMAGE_CAPTIONED, "hidden", Some(&format!("The generated scene image shows: {caption}")), &json!({"asset_id": asset_id, "model": model, "caption": caption}), Some(<same target as persist_image_record>), Some(&target.turn_id))`.

  **+20.**
- **Tests,** using the existing `turn_fixture`:
  - `persist_caption` writes one `image_captioned` row with the asset id, caption and turn id, visible only after commit.
  - A failing `persist_caption` inside `with_savepoint` (a missing source action) leaves the image rows and the turn intact.

  **+40 to +50 test lines.**

**The network call itself has no unit test.** It's covered by QA.

### Captions reach the UI and are cleaned up
**`src-tauri/src/features/transcript/model.rs` → `StoryImage`:** add `#[serde(default)] pub caption: Option<String>`. Every constructor sets `caption: None`: `generation.rs`, the `row_to_image` path and the tests. `rg -n "StoryImage \{" src-tauri/src` lists the 6 sites. **+2**, plus **+1** per constructor.

**`src-tauri/src/features/transcript/attachments.rs`:**
- **`images_for_story`:** select the caption as a subquery, `(SELECT json_extract(c.payload_json, '$.caption') FROM transcript_entries c WHERE c.kind = ?2 AND json_extract(c.payload_json, '$.asset_id') = image_assets.id LIMIT 1)`, bind `IMAGE_CAPTIONED`, and read it in `row_to_image`. **+3.**
- **`detach_from_entry`:** the event query becomes `WHERE kind IN (?1, ?2)` with `IMAGE_GENERATED` and `IMAGE_CAPTIONED`, so detaching an entry also deletes its caption records. **+1.**
- **Tests:**
  - Extend `detaching_an_entry_removes_only_its_images_and_events` with an `image_captioned` row for `asset-1`. It must be deleted, and the other asset's caption kept.
  - Add one test that `images_for_story` returns the caption for a captioned image and `None` for an uncaptioned one.

  **+20 test lines.**
- **`transcript/erase.rs` needs no change.** Erase and Retry remove a turn's entries by `turn_id`, and the caption carries the turn id. **Confirm** this with a test, or by reading `remove_turn`, and note it in the report.

**Frontend** (still in C2, so types and backend change together):
- **`src/shared/types.ts`:**
  - `StoryImage` gains `caption: string | null`.
  - `TranscriptEntryKind` gains `"image_captioned"`.
  - Add `export interface ImageCaptionedPayload extends TranscriptPayloadBase { asset_id: string; model: string; caption: string }`, and a matching union member in `TranscriptEntry`.

  **+3.**
- **`src/features/transcript/TranscriptEntryView.tsx`:**
  - `ImageCaption` takes `{ prompt, caption }`.
  - The button says "Image details" when a caption exists, and "Image prompt" otherwise.
  - When opened with a caption, it shows two short labelled blocks, **"What was drawn"** (the caption) and **"Prompt"** (the prompt), in the existing box style.
  - The `<img alt>` uses `image.caption ?? image.prompt`.

  **+8 to +12.**
- **`src/features/transcript/TurnActivity.tsx` needs no change.** `TOOL_EVENT_KINDS` is for legacy turns only.
- The TS test fixtures that build a `StoryImage` get `caption: null`, if `tsc` asks for it.

**Commit C2:** "Caption each generated image with a vision model; Image captions toggle". **Production about +150 to +175, tests about +150 to +170.**

---

## C3. QA in the running app (tauri-pilot, no commit unless a fix is needed)

Start the `image-captions` build with `pnpm tauri dev`. Save every probe.

1. **Settings.**
   - **Open Image Model.** Pass if a "Captions" section contains, in order, a checked "Caption images" checkbox and a "Caption model" select showing "Gemini 3.5 Flash Lite (default)". Save as `c3-01-settings.json`.
   - **Choose Custom… and type `mistralai/mistral-nemo`, which is text-only, then Save.** Pass if an error naming the model appears, and the stored row (read-only) still has the old `caption_model`.
   - **Set it back to the default and Save.** Pass if the row has `"caption_model":"google/gemini-3.5-flash-lite"`.
2. **Toggle.**
   - **New story, then open Context → Transcript.** Pass if "Image captions" is listed under Records, directly after "Image prompts", checked, and "Image prompts" is unchecked. Save as `c3-02-toggle.json`.
3. **Captioned Do turn.**
   - **Submit "Look around the room, and illustrate it."** If the narrator makes no image, use See ("the room") instead.
   - **Pass if all of these hold:**
     - exactly one `image_captioned` row exists for the new image's `asset_id`, with a non-empty `caption`, the same `turn_id` as the `image_generated` row, and `visibility = 'hidden'`
     - one `usage_records` row with `kind = 'caption'` has that `turn_id`, and its `cost_usd` is not NULL (a NULL is recorded, not failed)
     - the image's "Image details" disclosure shows "What was drawn" with the same caption text
     - the header's image count went up by 1 and not by 2

     Save as `c3-03-captioned.json`.
4. **Sent to the narrator.**
   - **Open the context preview.** Pass if it contains `[Authoritative story event: image_captioned]` followed by that caption, and **no** `image_generated` line. Save as `c3-04-preview.json`.
   - **Uncheck "Image captions".** Pass if the preview no longer contains the caption line. Save as `c3-04b-preview-off.json`.
5. **With the toggle off, captions are still made; they just aren't sent.**
   - **With the toggle still off, run one See ("the door").** Pass if all of these hold:
     - the new image **has** an `image_captioned` row and a new `caption` usage row
     - its disclosure shows "What was drawn"
     - the context preview contains **no** `image_captioned` line

     Save as `c3-05-off.json`.
5b. **"Caption images" off means no caption is made.**
   - **Turn the story's toggle back on. In Image Model, uncheck "Caption images" and Save.** Pass if the caption model picker becomes disabled and keeps its value, and the stored row has `"captions_enabled":false`.
   - **Run one See ("the floor").** Pass if the new image has **no** `image_captioned` row and **no** new `caption` usage row, and its disclosure says "Image prompt".
   - Save as `c3-05b-captions-off.json`. Check "Caption images" again and Save afterwards.
6. **Caption failure keeps the image.**
   - **Set the caption model to Custom `openai/does-not-exist`.** The lookup finds nothing, so it saves with a warning.
   - **Run one See ("the window").** Pass if:
     - the image appears
     - there's no `image_captioned` row for it
     - the log has one caption warning and no `ERROR` or `panicked`

     Save as `c3-06-failure.json`.
   - Restore the default caption model afterwards.
7. **Retry and Erase clean up.**
   - **Click Retry on the reply from check 3.** Pass if the old `image_captioned` row is gone along with its image. Save as `c3-07-retry.json`.
   - **Then click Erase.** Pass if no `image_captioned` rows remain for that turn.
8. **Restart.**
   - **Stop and start the app, then reopen the story.** Pass if the surviving captions still show under their images. Save as `c3-08-restart.json`.
9. **Logs.** Nothing unexpected. Save as `c3-09-logs.txt`.

Stop the app afterwards. Leave the stories in place for the user.

## C4. Merge

If every check passed, fast-forward `main` to `image-captions` (`git merge --ff-only image-captions` on `main`). **Don't push.** If any check failed, leave `main` alone and say so at the top of the report.

## The report (`docs/report/image-captions.md`)

- A results table: check, pass/fail, evidence file.
- The actual line counts per commit, next to this plan's estimates.
- Every choice made along the way, including:
  - which Rig call `describe_image` uses
  - how the save error is shown
  - the `erase.rs` confirmation
- Three real captions from QA, next to the prompt each image was made from, so the user can judge whether captions add information the prompt lacked.
- The measured cost per caption.

---

## Line count estimate

These are production lines, based on the code at `9f6117d`. Tests are counted separately.

| Step | Production | Tests | Where the lines come from |
|---|---|---|---|
| C0: widen the CHECK by hand | 0 | 0 | one-off script outside the repo |
| C1: caption settings, backend | +33 to +36 | +15 to +20 | constant, two fields, read/write with the empty-means-default rule, `openrouter_accepts_images`, save validation |
| C1: caption checkbox and model select, frontend | +36 to +46 | 0 | checkbox, list, select plus custom input, help line, state |
| C2: record kind and toggle | +3 | +16 | constant, `RECORD_KINDS`, label; history test plus default assert |
| C2: `describe_image` and `caption_prompt` | +37 to +42 | +8 | Rig vision call, prompt text |
| C2: usage kind and cost sums | +14 | +12 | `Caption` variant, `UsageRecord::caption`, 4 SQL edits in place |
| C2: `caption_image`, `persist_caption`, wiring | +67 to +74 | +40 to +50 | the call, the timeout, usage, the record, the `captions_enabled` check |
| C2: `StoryImage.caption`, `images_for_story`, detach | +12 | +20 | field plus 6 constructors, subquery, `IN` list |
| C2: types and image disclosure | +11 to +15 | 0 | types, "What was drawn" block |
| **Total** | **about +213 to +242** | **about +111 to +126** | |

**Where the lines could be cut, and why they aren't:**
- **Dropping the selectable model** and hard-coding one would save about 40 lines. The user chose selectable.
- **Recording caption cost as `image`** would skip C0 and the new usage kind, about 14 lines. But it would count every caption as a second image in the header.
- **Captioning after commit instead of inside the turn** would avoid the 2–4 s it adds. It would need its own background writer, Retry and Erase cleanup, and a race with the next turn, which is more code than it saves.
