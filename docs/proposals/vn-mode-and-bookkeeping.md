# VN Mode, Consistent Images And Bookkeeping

> **Superseded (2026-09-30)** by [vn-mode-and-consistent-images.md](vn-mode-and-consistent-images.md): the narrator does the bookkeeping, cards get typed tables, images become a general engine, and the work is split into phases.

A design report, 2026-09-30. **Nothing was built or generated for it.** It sums up a planning conversation, checked against the code on branch `turn-costs` (`edf77f8`) and against OpenRouter's public model list. No images were generated, so every claim below about how consistent a model is remains untested until the image probe in phase 3 runs.

**Verdict.** A static VN mode, with consistent characters and backgrounds, is a moderate job. It does not animate anything. Each turn shows one complete scene image with the text over it. Consistency comes from **stored reference images** sent with every image request, not from seeds or sprite compositing. The part that has to be reliable is **bookkeeping**: an outfit change or a new character that is never recorded makes the reference images wrong. So bookkeeping moves from the narrator into a **separate bookkeeper pass** that runs every turn. The one open risk is whether the image model keeps identity across 2–4 references. That is a question about the model, not the code, and a probe can answer it before any app work starts.

## What Exists Today

| Piece | Where | Relevance |
| --- | --- | --- |
| `illustrate_scene` takes `description` + `character_ids`, one image per turn | [tools.rs:62-110](../../src-tauri/src/features/narrator/tools.rs#L62-L110) | Characters are already linked to images, as text only. |
| The appearance anchor is added to the image prompt as text | [generation.rs:32](../../src-tauri/src/features/images/generation.rs#L32) | The only consistency mechanism today. |
| `characters_by_ids` drops characters with no `appearance_anchor` | [generation.rs:60](../../src-tauri/src/features/images/generation.rs#L60) | Must change once a card can have an image without anchor text. |
| The image request sends only `model` + `prompt` | [openrouter.rs:24](../../src-tauri/src/features/images/openrouter.rs#L24) | Needs `input_references` and `aspect_ratio`. |
| `image_assets.entry_id` is `NOT NULL`, pointing at a transcript entry | [db.rs:157-164](../../src-tauri/src/shared/db.rs#L157-L164) | Card/location/item images need a different owner. |
| `create_entity` / `update_entity` tools; one `appearance_anchor` string per entity | [prompts.rs:173-200](../../src-tauri/src/prompts.rs#L173-L200) | Bookkeeping is optional and is done during narration; nothing checks it. |
| The `<entities>` block is injected every turn, and user overrides win | [blocks.rs:37-69](../../src-tauri/src/features/context/blocks.rs#L37-L69) | Already carries cards into later turns. |
| Entity changes are recorded as events tied to the turn | `features/entities/projection.rs` | The pattern to reuse for outfit and location state. |

Entity kinds are free-form strings. `character`, `object` and `location` are already named in the tool schema.

## Scope

**In scope:** a consistent background for each place; characters in varied poses, positions and expressions, and in outfits that change with the story; notable items; single-shot scenes such as "the crimson throne".

**Out of scope:** animated entrances, exits and slides; animated sprites (walk cycles, gestures); layered sprite compositing. These were considered and dropped. Transitions need a director that emits stage cues and stage state that survives retry, edit and erase, which is a much larger job for something that isn't wanted.

## Consistency Approach

| Approach | Verdict | Reason |
| --- | --- | --- |
| **Seed numbers** | No | A seed reproduces an image only for the *same* prompt. A different prompt with the same seed gives a different face. Useful only for re-rolls and reproducing bugs. |
| **Sprite generator + background generator + compositing** | No | One sprite is needed for every pose × expression × outfit combination. Characters can't interact with the scene (sit on the throne, hold the sword). It needs transparent output or background removal, and lighting and perspective won't match. It gives pixel-identical backgrounds, which isn't worth that cost. |
| **Stored reference images, and the model composes the whole scene** | **Yes** | Varied poses and interaction come free. The references are stored assets independent of the model, so switching models keeps them. |

The approach in short: **generate the references once, store them, and send the relevant ones with every scene image.** Each scene prompt labels its references ("image 1 is Mira, image 2 is the throne room") and describes pose, expression, position and objects.

**Accepted trade-off:** backgrounds are "the same room with slight drift", not identical pixels. The model redraws the room each time.

## Image Models

OpenRouter's `/api/v1/images` accepts `input_references` (HTTP or base64 data URLs), `aspect_ratio`, `resolution` and `background`. How many references each model accepts varies and is not published.

From `GET /api/v1/models?output_modalities=image` (fetched 2026-09-30):

- **Grok (currently used):** `x-ai/grok-imagine-image-2.0` and `x-ai/grok-imagine-image-quality` both accept `text` + `image` input, so they can take references. The reference limit, identity preservation and clean-white-background behaviour are **unknown**.
- **Baseline to compare:** `google/gemini-3.1-flash-image` (the code's default is its `-preview` variant). It has a strong reputation for keeping a character consistent from a reference, but that is untested here.
- Other models that accept image input exist (Seedream 5, FLUX.2, GPT Image, Qwen Image 3). They are candidates if Grok and Gemini both fall short.

## Bookkeeper

### Why bookkeeping fails today
The narrator must remember to call `create_entity` / `update_entity` while it writes prose, and nothing checks it. Four things go wrong:
- It is a side task, so it gets skipped.
- `update_entity` replaces the whole anchor, so recording an outfit change means rewriting the full description.
- On providers that write tool calls out as text (Grok, Hermes on Ollama), the bookkeeping is lost silently.
- Outfit changes then go unnoticed, and the stored reference images keep showing the old outfit.

### Design
1. **Narration** finishes as it does today.
2. **Bookkeeper call.** A cheap model gets the new passage plus the current cards (id, name, stable look, current state) and returns **structured output**, which runs every turn and is not optional:
   `{ new_entities, outfit_changes, location_change, item_changes, present_in_scene }`
3. **The backend validates and applies it in code:**
   - Names are matched against existing entities (a case-insensitive unique name index already exists).
   - Changes are recorded as entity events tied to the turn, so retry and erase undo them.
4. **Image generation runs last.** It uses `present_in_scene` and the *updated* state instead of the narrator's `character_ids`.

The narrator keeps `get_entities`, attributes and `roll_check`, which it needs during the scene. Cost: one small call and about 1–3 s per turn.

### Per-kind tracking

| Kind | Stable part (fixes the reference image) | Changing state |
| --- | --- | --- |
| **Character** | Face, hair, build | Current outfit (from an outfit library); present in the scene or not |
| **Location** | Architecture and key features | Which location the scene is in (one pointer per story); lasting changes ("the hall is burned"). Sub-locations are separate entities, optionally linked by a parent field. |
| **Item** | Appearance | Who or where holds it (a character or location id); state ("broken") |

Time of day and weather are prompt text, not state. "The throne room at night" reuses the throne-room reference.

### Card split, outfit library, reference versioning
- **Split the anchor:** `appearance_anchor` becomes the **stable look**, and **outfit** becomes its own field.
- **Outfit library:** `outfits(entity_id, name, description, reference_asset_id)` plus a current-outfit pointer. When a character changes back into an earlier outfit, the stored image is reused, with no new generation.
- **Each reference image records which version of the text it was made from:**
  - When the text changes, the image no longer matches and is regenerated, either in the background or at the next scene that needs it.
  - An outfit change can't go unnoticed: the bookkeeper records it, the stale reference is caught in code, and the next scene uses the new outfit.
- **New outfit and expression images are made by *editing* the base identity image** ("same character, now wearing …"), so the face carries over.

### How to know it works
- **Visible:** a small line under each reply ("Mira: outfit → crimson gown · New: Captain Varro"). The user corrects it on the card, and user overrides already win.
- **Test set:** 20–30 short passages, each with the diff you expect. Include the traps:
  - thinking about an old dress (no change);
  - an unnamed "guard" (no card);
  - a nickname (no duplicate card);
  - an outfit change off-screen;
  - a passing mention of a place (no location change);
  - an item handed over (holder changes).

  Score each candidate model on what it misses and what it adds wrongly. This set chooses the model and catches prompt regressions.

## White-Background Reference Images

Characters and items are generated on plain white. This makes the best reference image, because no background leaks into scenes, and the same image can serve as a sprite later.

- **Characters:** full body, front-facing, neutral pose, feet visible, fixed aspect ratio (2:3 or 9:16), no floor shadow. Outfit and expression variants are made by editing this base image, also on white.
- **Items:** a product-style shot on white.
- **Locations:** the reverse. Show the empty place **with no people**, or those people appear in every scene set there.
- **Off-white output:** models often return off-white, a gradient or a soft shadow. Normalize with a Rust step using the `image` crate: flood-fill inward from the edges and set near-white to pure white.
- **White clothing and silver hair:** a global white key would punch holes in them. Filling from the edges mostly avoids this. Use a light grey background for characters dressed in white.

## Jev As An Optional Filter

[Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev) (TypeSafe AI, limited early access since 2026-09-15) returns typed decisions, not text:
- **Choice:** pick one option from a list.
- **Score:** rate against levels you define.
- **Noul:** the probability that a yes/no statement is true.

Several questions can go in one call. TypeSafe quotes roughly 70–500 ms and about $0.001 per decision.

**Good fit** (runs before the LLM bookkeeper):
- "New named character not on this list?" (Noul)
- "Did X's clothing change?" (Noul, one per character present)
- "Is X in the scene?" (Noul)
- "Who is 'the captain'?", with entity ids or *new* as options (Choice); this prevents duplicate cards for nicknames
- "Which known outfit is X wearing?", with *new outfit* as an option (Choice)
- "Did the scene move?" (Noul)

**Can't do:** writing names, looks or outfit descriptions. That stays with the LLM bookkeeper, which then runs only when a question comes back yes.

**Cautions:**
- **A missed yes is silent,** so keep the threshold low and treat low-confidence answers as yes.
- **The savings are small for one user.** The real gains are latency and calibrated confidence.
- **It's a new vendor outside the current provider scope,** and story text leaves the machine.
- **No Rust SDK:** it would need a small reqwest client written by hand.
- **Unknowns:** endpoint details and early-access terms weren't in the public docs; confirm access first.

**Rule:** turn it on only if it catches introductions and outfit changes, scored on the same test set, at least as reliably as the LLM bookkeeper. If it's down or unconfigured, the bookkeeper runs every turn.

## Risks

- **Identity drift with several references:** background + 2–3 characters + 1 item is where models confuse who is who. This is unknown for Grok; phase 3 measures it.
- **Too many references:** requests may exceed what the model accepts. Order them by priority (location, characters present, items named in the description) and cap at the model's limit.
- **The bookkeeper adds or misses things:** the test set, the visible change line and user overrides guard against this.
- **Cost:** each new character, outfit, location or item costs one reference image the first time; after that the stored image is reused. Scene images stay at one per turn.
- **Card images must not reach the text model:** `images_for_entries` sends narration images to the text model. Card images avoid this automatically as long as they aren't owned by transcript entries.

## Open Decisions

1. **Which unnamed characters and items get cards:** only named ones, or also recurring or plot-relevant ones? This must be written into the bookkeeper prompt.
2. **The model for the bookkeeper:** chosen by test-set score and cost.
3. **The image model:** Grok if it passes the probe, otherwise the best alternative. The design stays the same either way.
4. **Whether to try Jev,** once early access is confirmed.

## Proposed Phases

| Phase | Work | Rough size |
| --- | --- | --- |
| 1 | Bookkeeper pass for characters, locations and items; card split (stable look / outfit); location pointer and item holder as entity events; visible change line; **test set** | Medium |
| 2 | Reference images: new owner table (for example `entity_images(entity_id, asset_id, role, source_version)`) created by hand, with no startup migration; white-background generation and cleanup; outfit library; stale detection; card UI (generate, upload, promote a scene image) | Medium |
| 3 | **Image probe**, a read-only script run before phase 4: 1 location + 2 characters + 1 item, about 6 prompts varying pose, expression, outfit and interaction; Grok 2.0, Grok quality and Gemini Flash Image; record the reference limit, drift, white-background quality, cost and time; save every output plus a REPORT.md | Small |
| 4 | Scene images from references: `input_references` + `aspect_ratio` in `openrouter.rs`; references ordered and capped; labelled prompt; `present_in_scene` replaces narrator `character_ids` | Small |
| 5 | VN view: the latest scene image full-screen with a text box; no transitions | Small–medium |
| 6 | Jev filter (optional), scored on the phase 1 test set | Small |

Phase 3 can run in parallel with phase 1, since it needs only hand-made reference images.

## Sources

- [OpenRouter image generation docs](https://openrouter.ai/docs/features/multimodal/image-generation)
- OpenRouter model list: `https://openrouter.ai/api/v1/models?output_modalities=image`
- [Introducing System One Models & Jev](https://typesafe.ai/blog/introducing-system-one-models-and-jev), [TypeSafe docs](https://docs.typesafe.ai/introduction), [Jev on Wikipedia](https://en.wikipedia.org/wiki/Jev_(AI_model)), [The Register on Jev](https://www.theregister.com/devops/2026/09/23/shut-up-and-calculate-jevs-new-ai-primitives-for-coders/5298431), [Cloudflare AI: Jev](https://developers.cloudflare.com/ai/models/typesafe/jev/)
