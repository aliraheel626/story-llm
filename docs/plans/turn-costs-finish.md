# Finish turn-costs: the Grok text leak, an early-failure race, in-app QA, then merge

## Context

The branch `turn-costs` is at `9755af5`. It isn't merged yet. The Codex run (report: `docs/report/turn-costs.md`) passed every check except the live image timer, which the user has since confirmed by hand. Claude then added three commits:

| Commit | What it does |
|---|---|
| `d71eb92` | **See turns stream no text** (`narrator/stream.rs`, the `NarratorChunk::Text(_) if stop_after_tool_result` arm). A See turn never saves text anyway. |
| `1747fb4`, `9755af5` | **Each reply's text cost now sits right under its text,** before any images. Each image keeps its own `$0.0683 · 10.7 s` caption. The bottom row only has Edit/Retry/Erase. The tooltip on the text cost gives the turn total with images and the earlier-attempts note. The user chose this layout. |

**The Grok leak. Proven upstream, not Rig.** A raw OpenRouter stream (upstream provider: xAI, model `x-ai/grok-4.7`) carries the `illustrate_scene` arguments **both** in `delta.content` and in `delta.tool_calls`. Rig routes both correctly. The text copy even differs from the real call: it has `character_ids`, and the call doesn't.

Claude's See-turn prompt test is in `docs/probes/see_variants_results.jsonl`. There were 35 runs; each row is a variant, and the columns show how many runs came out each way:

| Variant | Clean | Tool-call JSON or `<tool_call>` in text | Story prose in text | No tool call |
|---|---|---|---|---|
| Current wording ("never write the call out as text"), tool forced (`required`) | 0/5 | **5/5** | 0 | 0 |
| **That wording removed, tool forced** ("no_mention") | 6/9 | 1/9 | 2/9 | 0 |
| "Whole reply is the call; leave text empty", tool forced | 2/3 | 1/3 | 0 | 0 |
| Current wording, forced by name | 2/3 | 1/3 | 0 | 0 |
| Wording removed, forced by name | 2/6 | 1/6 | 3/6 | 0 |
| Current wording, `auto` | 2/3 | 0 | 1/3 | 0 |
| Wording removed, `auto` | 4/6 | 0 | 1/6 | **1/6** |

**What the numbers say:**
- The warning itself seems to *prime* the JSON leak.
- No wording is clean every time. That's why `d71eb92` fixes See in code.
- **Untested:** normal turns. There, leaked text would be **saved into the story.**

**The early-failure race.** Codex's `fdb8136` handles a `narration-done` event that arrives before `submit_turn` / `retry_narration` returns its stream id. The same race exists for **`narration-error`**:
- `_fail` (`src/features/story/store.ts:512`) returns early when the stream isn't installed yet. The composer then stays locked until a restart.
- `NarrationErrorPayload` (`narrator/stream.rs:29`, `src/shared/types.ts:66`) carries only `stream_id` and `message`, so the frontend can't tell which story the event belongs to.

**Outcome:**
- The prompt wording is chosen by measurement on See **and** Do turns.
- An early failure no longer locks the composer.
- Everything on the branch is verified in the running app.
- `main` is fast-forwarded (not pushed).

## Ground rules

- **Start point:** the `turn-costs` branch at `9755af5`, with `git status` clean. `docs/` is git-ignored. Don't rewrite existing commits.
- **Run the whole plan in one go, without asking the user anything.** Where something is unclear, choose, and record the choice in the report.
- **Fix, don't stop.** When a check fails:
  1. Find the cause and fix it on `turn-costs`, with a unit test when it can be reproduced in one. Commit it as "Fix …", with its line counts and cause.
  2. Rerun the four checks, then the failed check and everything after it.
  3. Up to **5 attempts per check**, within the spending cap. If a check still fails, record it with what you tried, carry on, and **don't fast-forward `main`**.
  - Environment problems (a stuck process, a locked file) get an environment fix.
- **No migration code in the app.** This plan changes no schema. Before each commit, `rg -n "ALTER TABLE|pragma_table_info|table_info|migration" src-tauri/src` must have hits only in `#[cfg(test)]` code.
- **Database and secrets:**
  - Destructive database changes are allowed, but none should be needed.
  - **Never** open, print, copy, overwrite or delete `secrets.json`.
  - The probe scripts read the API key from `secrets.json` **into memory only**. They must never print, log or write it. Keep that property in any script you write.
  - **Never push.**
- **One commit per code step** (F2, F3, plus any "Fix …"). Each message states its production and test line counts from `git diff --numstat`.
- **After every commit:**
  - `cargo test --manifest-path src-tauri/Cargo.toml`
  - `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` with zero warnings
  - `node --test src/features/story/store.test.mjs src/features/transcript/replacement.test.mjs src/features/usage/store.test.mjs`
  - `npx tsc --noEmit`
- **The report** goes to `docs/report/turn-costs-finish.md`, and it links each evidence file by its full path.
- **Evidence files** go in `%TEMP%\codex\turn-costs-finish\`. A check without a saved file is "no evidence", never a pass.
- **QA drives the app like a user,** with real clicks and typing through tauri-pilot. Database reads are read-only (`mode=ro`). Never edit the DOM, app state or rows to make a check pass.
- **Spending cap:** at most **30 text model calls and 3 images** in total, including probes and fix reruns. That's about $0.35. Probes are direct OpenRouter calls, so each run counts as 1 text call.
- **tauri-pilot notes:**
  - Start the app with `pnpm tauri dev` (the `run` skill isn't available).
  - The pipe is `\\.\pipe\tauri-pilot-com.story-llm.app`. Drive it from **PowerShell**; Git Bash mangles the pipe path.
  - PowerShell 5.1 splits double-quoted arguments that contain spaces. Use here-strings, or prefix selectors like `[aria-label^=New]`.
  - Waiting for a state and then capturing it missed the live image placeholder last time. **Poll with read-only DOM captures in a loop** (every 300–500 ms) instead.

---

## F1. Measure the leak on a normal Do turn (probes only, no commit)

**Why:** the See rule and the `illustrate_scene` description are also sent on normal turns. There, leaked text is saved into the narration. The rewording must not make normal turns worse.

1. **Copy** `docs/probes/see_variants.py` to `docs/probes/do_variants.py`, and change it:
   - **Request:** the same system prompt and history, taken from the logged `<see>the pebble</see>` request in `%LOCALAPPDATA%\com.story-llm.app\logs\story-llm.log`.
     - The last user turn becomes `<additional_instructions>` naming `illustrate_scene` (reuse the logged one), followed by `<do>Pick up the pebble and turn it toward the window light.</do>`.
     - `tool_choice` is **`auto`**, as normal turns use.
     - Offer `illustrate_scene` only.
   - **If that log line is gone,** build the system prompt from `narrator_system_prompt()`'s text in `src-tauri/src/prompts.rs`, and write a one-paragraph narrator history yourself. Record that.
   - **Two variants:**
     - `current`: the wording exactly as it is now
     - `reworded`: the See rule without the sentence "Make it a real tool call through the tool-calling interface; never write the call out as text, and write no other prose.", and the tool description with "Invoke it as a real tool call, at most once per turn; never write the call out as text." replaced by "Call it at most once per turn."
   - **For each run, record:**
     - `called_tool`
     - `leaked_tool_text`: the content contains `{"description"`, `"character_ids"`, `<tool_call>` or `illustrate_scene(`
     - `has_prose`: at least 80 characters of content that isn't tool text
     - `cost`
2. **Run 5 runs of each variant** (10 calls). Save the output as `f1-do-variants.jsonl`, with a summary table in the report.
3. **Decision, recorded in the report:**
   - If `reworded` has **no more** `leaked_tool_text` runs than `current`, apply the full rewording in F2.
   - Otherwise, F2 removes **only the See-rule sentence** from the system prompt, and keeps the tool description as it is.
   - Either way, if any Do run leaked tool text, list it in the report as a follow-up: a guard that drops tool text from saved narration. **Don't build that guard in this plan.**

## F2. Reword the prompt

**File:** `src-tauri/src/prompts.rs`.
- **The See rule** in the system prompt (about line 51): delete the sentence "Make it a real tool call through the tool-calling interface; never write the call out as text, and write no other prose." Keep "Never skip it." and the rest.
- **Only if F1 says so:** in `ILLUSTRATE_SCENE_DESCRIPTION` (about line 103), replace "Invoke it as a real tool call, at most once per turn; never write the call out as text." with "Call it at most once per turn."
- **Leave these unchanged:**
  - the closing line "Your replies are only narration or real tool calls."
  - `ToolChoice::Required` for See (`ai/mod.rs:441`)
  - `d71eb92`
- **Fix any test** that asserts the old wording. `rg -n "never write the call out" src-tauri/src` must show no hits afterwards.

**Confirm on See:**
- Rerun `docs/probes/see_variants.py` against the **new** wording, 6 runs. Give it a variant that reads the edited `prompts.rs`, or edit the script's `SEE_OLD`/`DESC_OLD` handling so `baseline` equals the new text.
- Save the output as `f2-see-confirm.jsonl`.
- **Pass if** JSON or `<tool_call>` leaks in at most 2 of 6 runs. Anything above that is recorded, not a failure: `d71eb92` already hides See text.

**Line estimate:** production about −2/+1. **Commit:** "Stop priming Grok to write tool calls as text".

## F3. An early `narration-error` must not lock the composer

**Backend** (`src-tauri/src/features/narrator/stream.rs`):
- `NarrationErrorPayload` gains `story_id: &'a str`.
- `emit_error(app, stream_id, story_id, error)` passes it. Its one caller is at about line 230, after the `async` block; `story_id` from the destructured `Prepared` is still in scope there.

**Frontend:**
- **`src/shared/types.ts`:** `NarrationErrorPayload` gains `story_id: string`.
- **`src/app/useNarrationEvents.ts:31`:** pass the story id, so `_fail(streamId, message, storyId)` gets it.
- **`src/features/story/store.ts`,** mirroring `fdb8136`:
  - Add `const earlyFailures = new Map<string, { stream_id: string; message: string }>();`.
  - **In `_fail`,** when `findStream` finds nothing and `bundles[storyId]?.requestPending` is true, store the failure and return.
  - **In `submitTurn` and `retryNarration`,** right after the early-completion replay, add the same for failures: `get()._fail(...)` when the ids match. Clear the entry in the `catch` too.

**Tests** in `src/features/story/store.test.mjs`, copying the two early-completion tests (`early See completion…`, `early Retry completion…`):
- `early See failure does not leave the composer streaming`
- `early Retry failure restores the original reply`
- **Assert:**
  - `streaming === null`
  - `requestPending === false`
  - `turnError` is the message
  - for Retry, the original entry is still there

**Line estimate:** production +12 to +18, tests +40 to +50. **Commit:** "Fix early narration-error leaving the composer locked".

---

## F4. QA in the running app (tauri-pilot)

Start the `turn-costs` build with the normal identifier, `com.story-llm.app`. Use a new story: click "+ New story", and keep all narrator tools on, including Illustrate scenes. Save every probe.

1. **One Do turn with images on:** submit "Look around the room." and wait.
   - **Pass if:**
     - The saved narration in `transcript_entries.content` contains none of `{"description"`, `"character_ids"`, `<tool_call>` or `illustrate_scene(`.
     - A text cost appears **directly after the text paragraph** in DOM order, before any image or roll. Its value equals `text_cost_usd` for that turn from `get_story_usage_breakdown`.
     - The bottom action row contains no `$`.
   - If the narrator made an image on its own, it counts toward the image cap.
   - Save as `f4-01-do.json`.
2. **See, the leak and the timer:** select See, type "the window", and submit.
   - **While the image placeholder is visible,** poll every 300–500 ms with read-only captures of:
     - the streaming reply paragraph's text
     - the placeholder label
   - Keep at least 3 captures.
   - **Pass if:**
     - The streaming paragraph never contains `{`, `description` or `<tool_call>`.
     - The timer label's number increases across two captures at least 1 s apart.
   - Save as `f4-02-see-stream.json`.
3. **The composer unlocks after See** (tests `fdb8136`).
   - **Pass if,** within 2 s of the image appearing:
     - the composer textarea is enabled
     - the submit button is enabled and doesn't say "Generating…"
   - Then submit a short Do ("Step back from the window.") to prove it accepts input.
   - Save as `f4-03-unlock.json`.
4. **See image cost placement:**
   - The See image appears under the previous reply, with its caption `$… · … s`.
   - That reply's text cost is **unchanged,** because the See turn's cost isn't added to it.
   - The caption tooltip contains "See turn total".
   - Save as `f4-04-see-placement.json`.
5. **Retry:** click Retry on the last reply, and wait.
   - **Pass if:**
     - the replacement's text cost equals `text_cost_usd` of its turn, which now includes the earlier attempt's text
     - its tooltip says "Includes $… from earlier attempts (Retry)."
   - Save as `f4-05-retry.json`.
6. **Restart:** stop and start the app, then reopen the story.
   - **Pass if** every text cost, image caption and the header are identical to before.
   - Save as `f4-06-restart.json`.
7. **Logs:** no unexpected `ERROR` or `panicked` for this run. Save as `f4-07-logs.txt`.

**Not tested in the app:** the early-failure race (F3). It can't be triggered on demand; the unit tests cover it.

Stop the app afterwards. Leave the story in place for the user.

## F5. Fast-forward `main`

- **Only if F4 passed** (after fixes):
  - `git checkout main`
  - `git merge --ff-only turn-costs`
  - save `git log --oneline -12` as `f5-main.txt`
  - **Don't push.**
- **If `main` has moved:** merge `main` into `turn-costs`, rerun the four checks and F4 checks 1 and 6, then fast-forward.
- Leave `main` checked out with a clean tree.

---

## Final message and the report (`docs/report/turn-costs-finish.md`)

- Whether `main` moved, and to which commit.
- **F1:** the Do-turn table, and the decision with its reason.
- **F2:** the See confirmation table, next to Claude's earlier numbers.
- **F3:** the commit, and its tests.
- Each F4 check: pass or fail, with its evidence file.
- Every "Fix …" commit with its cause.
- Model calls and images used, and their cost.
- Every commit with its measured line counts.
- The migration grep for each commit.
- Follow-ups: the normal-turn guard (only if F1 found leaks), and anything else noticed.

## Line count audit

| Step | Production | Tests |
|---|---|---|
| F1 probes | 0 (git-ignored `docs/probes/`) | 0 |
| F2 prompt wording | about −2/+1 | about ±2 if a test asserts the wording |
| F3 early `narration-error` | +12 to +18 | +40 to +50 |
| **Total** | **about +10 to +17** | **about +40 to +52** |

## Not in this plan

- **A guard that drops tool text from saved narration.** Only worth building if F1 finds leaks on Do turns.
- **Hiding "Cache write" in the header when it's 0.** It isn't a bug: xAI reports `cache_write_tokens: 0` while `cached_tokens` is non-zero, because its caching is automatic.
- **Placing images between paragraphs.**
- **`AGENTS.md` and the planning skill.**
