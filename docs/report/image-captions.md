# Image Captions Implementation And QA

**Not merged into main.** C1 and C2 are implemented and committed on `image-captions`, and every automated check passed after both commits. Live Retry QA is **unverified**, not a pass: all four allowed images were used by checks 3, 5, 5b and 6. Check 3 used the permitted See fallback, so retrying its image-owning turn would necessarily generate a fifth image. The requested third real caption is also unavailable within the cap: the four-image sequence permits only two successful captions, one deliberately uncaptioned image and one deliberate caption failure.

The app is stopped. QA stories remain in the database. Global captions are on and the default caption model is restored. Nothing was pushed. `main` remains at `9f6117d1b708a324c0faec966ad18e1364de1b5e`; `image-captions` is at `71ef4b855ea9d75df86a8beb579a76135cdd126a` with a clean working tree. C4 was not run because its acceptance condition was not met.

## Results

| Check | Result | Evidence (full path) |
| --- | --- | --- |
| C0: backup, preserved row count, widened CHECK, integrity | PASS: 45 rows before and after; `caption` in CHECK; integrity `ok` | [c0-schema.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c0-schema.txt) |
| C1: cargo test | PASS | [c1-1.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c1-1.txt) |
| C1: Clippy, zero warnings | PASS | [c1-2.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c1-2.txt) |
| C1: specified Node tests | PASS | [c1-3.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c1-3.txt) |
| C1: TypeScript | PASS | [c1-4.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c1-4.txt) |
| C2: cargo test | PASS: 186 tests | [c2-1.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c2-1.txt) |
| C2: Clippy, zero warnings | PASS | [c2-2.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c2-2.txt) |
| C2: specified Node tests | PASS: 47 tests | [c2-3.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c2-3.txt) |
| C2: TypeScript | PASS | [c2-4.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c2-4.txt) |
| C3-01: caption controls, default model, order | PASS | [c3-01-settings.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-01-settings.json) |
| C3-01: text-only custom model rejected without changing stored model | PASS: visible error names `mistralai/mistral-nemo` | [c3-01-rejected.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-01-rejected.json) |
| C3-01: restore and save default | PASS | [c3-01-restored.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-01-restored.json) |
| C3-02: fresh-story caption default and record ordering | PASS: captions on immediately after prompts; prompts and image input off | [c3-02-toggle.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-02-toggle.json) |
| C3-03: captioned image, record/turn ownership, usage, disclosure, count | PASS using permitted See fallback; one image and one caption usage row, image count 1 | [c3-03-captioned.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-03-captioned.json) |
| C3-04: caption sent, prompt record absent | PASS | [c3-04-preview.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-04-preview.json) |
| C3-04b: caption excluded after story toggle off | PASS | [c3-04b-preview-off.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-04b-preview-off.json) |
| C3-05: new See image captioned despite story toggle off | PASS: caption record, usage and disclosure present | [c3-05-off.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-05-off.json) |
| C3-05: new caption excluded from narrator preview | PASS | [c3-05-off-preview.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-05-off-preview.json) |
| C3-05b: global off disables model picker and keeps its value | PASS | [c3-05b-settings-off.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-05b-settings-off.json) |
| C3-05b: global off prevents caption record and caption call | PASS; prompt-only disclosure | [c3-05b-captions-off.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-05b-captions-off.json) |
| C3-06: caption HTTP failure preserves image | PASS: image rendered, no caption row, one caption-request warning; no backend ERROR/panic | [c3-06-failure.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-06-failure.json), [backend excerpt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-06-backend-log.txt) |
| C3-06: restore default caption model | PASS | [c3-06-restored.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-06-restored.json) |
| C3-07: live Retry removes old caption/image | UNVERIFIED: no live evidence; fifth image prohibited | [cap state, not a passing Retry probe](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-07-retry-cap.json) |
| C3-07: live Erase removes captioned turn | PASS on the door See turn; earlier room caption preserved | [c3-07-erase-captioned.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-07-erase-captioned.json) |
| C3-08: stop/start and reopen, surviving caption persists | PASS: room caption equals its saved payload after restart | [c3-08-restart.json](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-08-restart.json) |
| C3-09: logs | PASS: expected model-rejection frontend error and deliberate caption-failure warnings only | [c3-09-logs.txt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-09-logs.txt), [backend excerpt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-06-backend-log.txt) |
| Report: three real captions | INCOMPLETE: only two successful captions allowed by specified sequence | [actual spending and caption counts](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/qa-summary.json) |
| C4: fast-forward main | NOT RUN: conditional acceptance not met | Main still `9f6117d`; feature HEAD `71ef4b8` |

No product check failed and required a Fix commit. The cap-limited Retry check is not represented as passing based on reading the code or on automated tests.

## Commits And Counts

These are actual additions/deletions from `git diff --numstat`, split at test-only code boundaries, not estimates of net growth.

| Step / commit | Plan's commit-section estimate | Actual production | Actual tests | Net production / tests |
| --- | --- | --- | --- | --- |
| C0 | 0 / 0 | 0 | 0 | 0 / 0 |
| C1 `f89492c` | Production +66 to +80; tests +15 to +20 | +95/-25 | +17/-0 | +70 / +17 |
| C2 `71ef4b8` | Production +150 to +175; tests +150 to +170 | +148/-21 | +252/-3 | +127 / +249 |
| C3 | No commit unless a fix is needed | 0 | 0 | 0 / 0 |
| Total | Final plan estimate: production +213 to +242; tests +111 to +126 | +243/-46 | +269/-3 | +197 / +266 |

The plan's C2 test estimate and its final test-total estimate differ; both are retained above rather than silently reconciled. Actual test coverage is larger: it covers serde defaults, toggle independence/restoration, same-story latest-caption selection, unrelated-caption preservation, usage metadata, story and turn aggregation, and caption-savepoint failure.

Commit messages include their measured counts. Counts and per-file numstat are saved in [C1 counts](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/numstat-f89492c.txt) and [C2 counts](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/numstat-71ef4b8.txt).

## Choices And Confirmations

- `describe_image` uses Rig's `agent.prompt(Message::User { content })` with a text part and a base64 image part, plus the existing `UsageHook`. The installed Rig accepts `Message` directly, so no lower-level completion workaround was needed. MIME validation uses `ImageMediaType::from_mime_type` before requesting a completion.
- The shared settings store previously rethrew save errors, while `useSettingsForm` only logged them. C1 adds a store `error` value and exposes it through the existing hook. `ImageModelPanel` renders that value as `role="alert"`; it does not introduce a separate local save-error state.
- Missing or blank caption model values normalize to the default before capability validation and again in repository storage. A definite text-only listing rejects before writing. Failed/unlisted metadata permits saving with a warning.
- Captions are requested only when global image generation is enabled and `captions_enabled` is true. The per-story record toggle is not consulted during generation.
- The 30-second caption timeout aborts its spawned task, records one unpriced caption usage entry and warns. HTTP, empty-output, join and persistence errors warn without failing the saved image. Captions are trimmed and capped using Unicode characters, not byte slicing.
- Reloaded `StoryImage.caption` comes from payload JSON, not event content: the event content has the narrator-facing prefix, while the image disclosure needs only the actual caption. Lookup chooses the latest same-story caption for the asset.
- `erase.rs` needs no production changes. `remove_turn` deletes assets, then the owning turn; transcript rows cascade by `turn_id`. Caption records carry that turn id. `prepare_retry` uses the same `remove_turn` path inside `TurnTx`. This is a code-path confirmation, not a substitute for live Retry evidence. Live Erase demonstrated removal of the door caption and image while retaining the room caption.
- No migration code was added. Before both commits the required `rg -n "ALTER TABLE|pragma_table_info|table_info|migration" src-tauri/src` command found only existing test-module hits: `shared/db.rs` test schema assertions and the `turn/retry.rs` test comment. C0 is a one-off ignored script; only the fresh-database CHECK changes in app code.
- C1 was verified in a detached temporary checkout at `C:/Users/User/AppData/Local/Temp/kilo/story-llm-c1`, because independently implemented C2 files were already in the shared workspace. Its four saved logs therefore test the actual C1 commit rather than mixed C1/C2 changes.
- The more specific C3 instruction, "no commit unless a fix is needed," was followed. Ignored QA scripts, evidence and this report were not force-added to Git.
- Every app change during QA used real pilot clicks, selects or typing. Database verification uses SQLite `mode=ro`. Pilot `eval` calls only read the DOM; they do not edit elements, app state or rows. No secret file was opened, printed, copied, overwritten or deleted by the maintenance/QA scripts.
- The existing preview UI truncates each message at 200 characters. After clicking "Preview next request," a read-only `preview_story_context` IPC query captures the full preview for checking the exact caption. The evidence retains both the clicked UI DOM and the full read-only query result.
- "New story" initially reused an existing empty draft with stored overrides. That draft became the QA story "The Door Without a Key." A genuinely fresh story was then created for defaults and feature checks, named "Turning in Place." Both remain. The reused draft's altered context defaults were not mistaken for new-story defaults.
- The first harness checkbox attempt used pilot `check`, which ensures checked state rather than turning it off. Subsequent toggles used real clicks. Optional entity/dice tools were then disabled in the fresh QA story to bound model calls. Saved harness evidence is retained, not relabeled as a product failure.
- One initial polling expression was mangled by PowerShell quoting, and a later predicate incorrectly expected the empty composer's Send button to become enabled. The harness was corrected to inspect the Continue button's readiness every 400 ms. Its old timeout evidence is retained. Neither mistake caused extra model calls.
- The permitted See fallback was necessary after the Do response made no image. Consequently the first caption belongs to a See turn, not the Do turn. See ignores the per-story illustration toggle; disabling that toggle cannot make its Retry text-only. Only the latest owning turn can be retried, so trying the original reply after later See turns would target a later See or be rejected. No fifth image was requested.
- For capped cleanup, the window and floor turns were erased first, then the captioned door turn. Restart QA uses the surviving room caption. The report does not claim the original first-caption turn was retried or erased.

## Actual Captions And Prompts

### 1. The Room

Prompt:

> Digital painting, atmospheric scene illustration. anime A narrow rented room seen from the middle of the floor at eye level, as if the viewer has just turned in place. Late-afternoon sun cuts through one tall window with a half-drawn linen curtain, laying a sharp gold bar across scuffed wooden floorboards and stirring dust in the air. An iron bed with a rumpled gray blanket stands against the left wall. A small wooden table holds a chipped enamel pitcher and a cold cup of tea. A dark coat is thrown over the back of a chair. Wallpaper the color of old paper peels at the seams. A clouded mirror hangs crooked above a washstand. Shadows pool in the corners; the window is the bright focal point. No people are visible.

Caption:

> A sunlit attic bedroom with peeling yellow-beige walls and a wooden floor features an unmade metal-framed bed against the back wall, covered in rumpled blankets. Sunlight streams in through a window on the right, illuminating dust motes in the air and casting bright squares of light on the floorboards. A wooden chair with a dark coat draped over it sits in the foreground left, while a small table with a pitcher and teacup stands next to the bed, and a washing stand with a basin and a towel hangs below a framed mirror on the right wall.

Reported caption cost: **$0.0006279**. It reports the drawn room's spatial arrangement and visible basin/towel rather than simply repeating the desired composition. Evidence: [first caption](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-03-captioned.json).

### 2. The Door

Prompt:

> Digital painting, atmospheric scene illustration. anime A tall heavy door of near-black vertical wood panels set into a dim interior wall, grain raised and scarred, a crooked pitted iron latch low on the door, a thin line of colder light leaking under the bottom edge, low warm lamplight from the room falling across worn floorboards, close composition centered on the door, no window, ominous and still

Caption:

> A dimly lit, heavy wooden door is set within dark stone walls and plank flooring. A lit lantern on the left casts a warm glow, while a narrow sliver of bright blue light shows underneath the closed door. The door itself is secured with a rusted metal latch.

Reported caption cost: **$0.0004779**. It identifies visible stone walls, a left-hand lantern and blue light, details not stated that specifically in the prompt. Evidence: [second caption](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-05-off.json).

### 3. Not Generated

There is no third successful caption to quote. The floor image intentionally has captions off, and the window image intentionally uses an invalid model. Inventing a third example or exceeding the four-image cap would violate the task. This reporting requirement remains incomplete.

## Spending

| Category | Actual calls | Allowed |
| --- | --- | --- |
| Text completions, including titles and the reused draft | 9 | 15 |
| Image generations | 4 | 4 |
| Caption requests | 3: two successes and one invalid-model HTTP error | 6 |

Total provider-reported cost: **$0.3261948**. Successful caption costs average **$0.0005529**. The invalid-model request returned HTTP 400 without completion usage and is counted in the request ledger, not fabricated as a billed usage row. Metadata lookups do not make model completions. Erasing images does not erase spending history. Evidence: [QA summary](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/qa-summary.json), [all calls before cleanup](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-06-failure.json).

## Supporting Evidence Index

All links below name full paths; these are supporting captures/scripts, not additional passing checks.

- [Initial story/settings/call baseline](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/baseline.json)
- [One-off C0 rebuild script](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c0-rebuild.py)
- [Database backup](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/db-backup/story-llm.sqlite3)
- [WAL backup](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/db-backup/story-llm.sqlite3-wal)
- [SHM backup](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/db-backup/story-llm.sqlite3-shm)
- [Commit verification runner](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/checks.py)
- [Line-count calculation](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/count-lines.py)
- [Pilot and read-only SQLite harness](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/qa.py)
- [Recorded direct interactions](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/interactions.jsonl)
- [Reused draft tracking](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/qa-story-ids.json)
- [Evidence audit and spending calculation](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/summarize.py)
- [Text-only rejection polling](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-01-reject-poll.json)
- [Default save polling](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-01-save-poll.json)
- [Fresh defaults before assertions](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-02-toggle-fresh.json)
- [Initial reused-draft Do with no image](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-03-do-no-image.json)
- [Superseded harness readiness timeout](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-03-poll.json)
- [Fresh-story Do readiness polling](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-03-poll-fresh.json)
- [See fallback readiness polling](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-03-see-poll.json)
- [First image before opening disclosure](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-03-before-disclosure.json)
- [Caption-with-story-toggle-off readiness polling](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-05-poll.json)
- [Global-caption-off readiness polling](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-05b-poll.json)
- [Invalid caption model saved](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-06-invalid-settings.json)
- [Failure-isolation readiness polling](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-06-poll.json)
- [Window turn erased](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-07-erase-window.json)
- [Floor turn erased](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/c3-07-erase-floor.json)
- [Initial checkbox harness attempt](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/tools-budget-1790797531.json)
- [Fresh-story optional tools disabled by real clicks](C:/Users/User/Documents/GitHub/story-llm/docs/report/image-captions/tools-budget-1790798012.json)
