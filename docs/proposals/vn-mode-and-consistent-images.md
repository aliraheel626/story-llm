# World Model, Image Engine And VN Mode: Proposal In Phases

A design proposal, 2026-09-30. It supersedes `vn-mode-and-bookkeeping.md`. It sums up the planning discussion, checked against `main` (`9f6117d`), OpenRouter's image docs and model list, and your database. Nothing has been built, and no image has been generated for it. Claims about how consistent the models are stay untested until phase 3.

## Decisions so far

1. **The narrator does the bookkeeping** through one tool, `record_changes`. There's no separate bookkeeper pass, unless the test set later shows the narrator misses too much.
2. **Typed cards with their own tables.** A shared base (identity, name, presence, events, attributes) sits under per-kind tables for characters, outfits, locations, items and relationships.
3. **"Think ahead" introductions.** The narrator writes the full card (true name, persona, goals, secrets, look, outfit) *before* the character appears. The prose may show only "a dark-hooded figure" (`known_as`) until the reveal. The UI shows the whole card, with no spoiler hiding.
4. **Images are a general feature with a per-story setting:** New every time, Reuse when possible, Stage (sprites over backgrounds), or Off. It works in the normal transcript and in VN mode. **See always makes a composed image,** and works even when "Illustrate scenes" is off (already on `main`, `9f6117d`).
5. **VN mode is a view,** not a separate engine. It shows the same images, with no animation.
6. **Later:** a world builder (seeded, or closed with a way to request new entities), and characters as sub-agents, which is a separate feature.
7. **Every on-screen character gets a card, at one of two levels:**
   - a **light card** for extras ("the guard", "three dockhands" as one card): `known_as`, look and outfit, with **no** persona, goals or secrets
   - a **full card** for named, recurring or plot-relevant characters

   An extra is **promoted** when it gets a name, recurs or matters. There's no toggle; the UI groups extras under a collapsed "Extras" heading.
8. **The default Images setting is "New every time"** (full composed generation).
9. **Stale assets regenerate automatically, but only when needed:** they're marked stale when a look or outfit changes, and regenerated the next time a scene uses them.
10. **The sprite set is 8 expressions × 6 standing poses per outfit,** made only when first used. Poses that involve the scene stay composed images. See phase 5.
11. **No spoiler hiding in the UI.** Cards show everything. `known_as` still shapes the prose and the change line ("the hooded figure (Varro)").
12. **The VN toggle is global.** VN mode is story mode plus frontend changes and a few prompt lines. Composed scenes are **always 16:9**, in both modes.
13. **Phase 2's acceptance bar** is set in phase 2: at least 90% of changes recorded, at most 1 false addition per 10 scenes, and no duplicate cards.

## Where things stand today

| Piece | Finding |
| --- | --- |
| Image model | **Gemini** `google/gemini-3.1-flash-image-preview` ([openrouter.rs:12](../../src-tauri/src/features/images/openrouter.rs#L12)). All 8 image rows are Gemini: about $0.07 each, taking 10–14 s. |
| The image request | Sends only `model` + `prompt` ([openrouter.rs:24](../../src-tauri/src/features/images/openrouter.rs#L24)). The API also takes `input_references`, `aspect_ratio` and `background: "transparent"`; transparency works on the GPT Image models. |
| `illustrate_scene(character_ids)` | **Barely works.** `<entities>` shows no ids ([blocks.rs:45](../../src-tauri/src/features/context/blocks.rs#L45)), and a See turn can't call `get_entities`. The pebble request had `"character_ids":[]`. |
| Entities | One generic table with a free-form `kind`, and a state row (name, `appearance_anchor`, `is_present`). Relationships are an entity *kind* with Trust/Fear/Affection/Respect attributes. `update_entity` rewrites the whole anchor. |
| Events and replay | `entities/projection.rs` (`record`, `replay`, `replay_after_erase`) rebuilds state from events tied to turns. **The pattern every new state change reuses.** |
| The Grok text leak | Grok writes tool calls out as text as well as making them (`docs/probes/`). See turns already hide it. **Normal turns would save it** once the narrator calls a tool most turns. |

## The data model (built in phase 1)

A shared base, with one table per kind. The tables hold **current state.** Everything that changes *during a turn* is also an **event** in the one event log, so Retry and Erase undo it by replay. Fields you edit by hand follow the existing rule that your overrides win.

| Table | Holds |
| --- | --- |
| `entities`, `story_entity_state` (exist) | **Shared:** id, kind (character / location / item / campaign), name, **`known_as`**, **`revealed`**, `is_present`. Keeps the name index, the `<entities>` block and attributes working for every kind. |
| `characters` | `tier` (`extra` or `full`), persona, goals, secrets (empty for extras), stable look (face, hair, build), `current_outfit_id`, `group_size` (for "three dockhands") |
| `outfits` | a character's outfit library: name, description |
| `locations` | look, `parent_location_id`, lasting-state note ("burned") |
| `items` | look, holder (a character or location), state note ("broken") |
| `relationships` | a link from entity A to entity B: type, note, and the Trust/Fear/Affection/Respect values. It replaces relationship-as-an-entity. |
| `story_scene` | the story's current location |
| `images` (phase 4) | every stored image with its **tags** (entity, outfit, expression, location, who was present, and whether it's a sprite, background or composed scene), plus a hash of the text it was made from |

**The schema change** alters existing tables. Under your rule it's applied by hand, by deleting the database. `secrets.json` stays.

---

## Phase 1: Typed world model

**Goal:** characters, places, items and relationships become real, typed cards, with state that survives Retry and Erase.

- The tables above, except `images`. The Rust models are `CharacterCard`, `LocationCard`, `ItemCard` and `Relationship`, and each kind's fields are validated in code.
- New event kinds, each with a replay handler:
  - `outfit_changed`, `location_changed`, `item_moved`, `state_noted`
  - `relationship_changed`, `presence_changed`, `revealed`
- The existing relationship entities and their attributes move to `relationships`.
- `<entities>` starts with a **Scene line**: "Scene: throne room · present: Mira (crimson gown), the hooded figure · Mira holds the brass key". Unrevealed characters appear under their `known_as` name, with their true card in a hidden section for the narrator.
- **Card UI** (frontend `characters/` and `world/`):
  - typed editors per kind
  - an outfit list
  - relationships
  - no spoiler hiding: the whole card is shown, with `known_as` next to the true name until the reveal

**Modules:** `entities` (models, repository, events, projection, commands), `shared/db.rs`, `context/blocks.rs`, frontend `characters/` and `world/`.
**Done when:** every event kind replays correctly on Retry and Erase (unit tests), the cards can be edited in the UI, and the Scene line shows in the preview.
**Rough size:** +700 to +950 production lines (backend about +500, frontend about +300).

## Phase 2: Narrator bookkeeping

**Goal:** the narrator keeps the cards up to date as part of each turn, and nothing it writes as tool text gets saved.

- **`record_changes`,** one call per turn at most, with every field optional and everything **by name** (true name or `known_as`, resolved in code):
  ```
  record_changes({
    introduce:     [{ name, known_as?, kind, persona?, goals?, secrets?, look, outfit? }],
    outfits:       { "<character>": "<outfit name or new description>" },
    location:      "<location>",
    present:       ["<character>", ...],
    items:         [{ name, holder?, state? }],
    relationships: [{ from, to, type?, note?, trust?, fear?, affection?, respect? }],
    reveal:        ["<character>"]
  })
  ```
  The tool result lists what was applied, plus any unknown or ambiguous names, so the narrator can fix them in the same turn. It replaces `create_entity`. `update_entity` stays for fixing a look or renaming.
- **The "think ahead" rule** in the prompt: introduce the card before any prose mentions the character, even if the prose only says "a hooded figure".
- **The card-level rule:** "Every character who appears gets a card. Extras get a light card (look and outfit only). Give a persona, goals and secrets only to characters who matter." `introduce` takes `tier: extra|full`. A later `introduce` of a known name with `tier: full` **promotes** the card, recorded as an event.
- **A change line under each reply:** "Mira: outfit → crimson gown · New: the hooded figure (Varro) · Scene → the docks".
- **The tool-text guard:** text that repeats a tool call (JSON matching a tool's arguments, `<tool_call>`, `name(…)`) is dropped before it's streamed and before it's saved.
- **The test set:** 20–30 scenes run through the real narrator prompt and tools, each with the changes you expect. Traps include:
  - an old dress remembered (no change)
  - an unnamed guard (no card)
  - a nickname (no duplicate)
  - an off-screen change
  - a passing mention of a place (no move)
  - an item handed over
  - a character leaving
  - a hooded figure revealed later

  It scores what was missed and what was added wrongly. It's a script in `docs/probes/`.

**Modules:** `narrator` (tools, catalog, prompts), `entities`, `transcript` (the guard), `turn` (the change line), frontend `transcript/`.
**Done when** the test set meets this bar, and no tool text appears in saved narration during a Grok QA run.
- **What the test set measures:** every scene has a list of the changes it should produce. A change the narrator doesn't record is a **miss**. A change it records that the scene doesn't justify is a **false addition**, for example a card for the unnamed guard's "mother", or an outfit change for a remembered dress.
- **The bar,** on the current narrator model (Grok 4.7), with the test set run twice and the results averaged:
  - at least **90%** of expected changes recorded
  - at most **1 false addition per 10 scenes**
  - **no duplicate cards** on the nickname traps
  - every character that appears gets a card at the right level (light or full)
- **If it falls short:** improve the prompt and the tool description, up to 3 rounds. If it still falls short, the separate bookkeeper comes back ("Later"), using the same `record_changes` schema.
**Rough size:** +350 to +500 production lines.

## Phase 3: Image probe (a script only, runs alongside phases 1–2)

**Goal:** decide the image model, and whether references and sprites work, before building phase 4.

- **Hand-made cards:** 1 location, 2 characters (one with two outfits), 1 item.
- **For each model, test:**
  - **Composed with references:** about 6 scenes varying pose, expression, outfit and interaction, including both characters together.
  - **Sprites:** a base sprite on a transparent background (or white, cut out), then 6 expressions and 1 outfit made by **editing** the base. Check style consistency and the edges around hair.
- **Models:** Gemini (current), Gemini non-preview, Grok Imagine 2.0 / quality, and a GPT Image model, which supports transparency.
- **Record:**
  - how many references each model accepts
  - identity drift
  - background leaking into scenes
  - transparency quality
  - consistency across expressions
  - cost and time per image
- **Output:** every image plus a report. Roughly 60 images, **$3–5**.

**Done when:** the report names a model for composed scenes and one for sprites (possibly the same), with evidence.

## Phase 4: Image engine and asset library (general, works in regular mode)

**Goal:** stored images, reuse, and consistent composed scenes, in the normal transcript.

- **The `images` table with tags.** Every composed scene, sprite, background and reference is saved with the scene state it was made under.
- **The per-story "Images" setting** in `settings_json`:

  | Setting | Ordinary turns | Special moments and See |
  | --- | --- | --- |
  | New every time | a fresh composed scene with references | composed |
  | Reuse when possible | an exact tag match (location · present · outfits · mood), otherwise a new composed scene | composed |
  | Stage | background + sprites (phase 5) | composed |
  | Off | none | See still composed |

- **Composed scenes use references:**
  - `input_references` + `aspect_ratio: "16:9"`
  - references picked from the recorded state: the location, then the present characters in their current outfits, then any items named, up to the limit phase 3 measured
  - a labelled prompt ("Image 1 is the throne room. Image 2 is Mira in her crimson gown…")
- **Lazy asset generation:** any missing or stale reference is made **after the turn commits,** in the background, so a first appearance never slows the turn. Outfit variants are made by editing the base image.
- **Stale assets regenerate automatically when needed:** a look or outfit change marks the affected assets stale, and each one is regenerated the next time a scene uses it, not all at once.
- **The default for new stories is "New every time".**
- **The card gallery:** generate, upload, "use this scene image", and **pick an old image** for the current scene. That's the manual way back to "the sad Mira".
- **`illustrate_scene(description, items?, mood?)`:** characters and location come from the recorded state, not from the call.

**Modules:** `images` (generation, openrouter, a new library), `narrator/tools`, `stories/settings`, frontend `transcript/`, `characters/` and `settings`.
**Done when:** in the running app, a second visit to the same scene reuses the image under "Reuse when possible", composed scenes keep identity at the level phase 3 measured, and nothing waits on reference generation.
**Rough size:** +700 to +850 production lines.

## Phase 5: Stage mode (sprites over backgrounds)

**Goal:** turns that cost $0 in images once the assets exist.

- **Stage directions** are added to `record_changes`: `stage: [{ name, pose, expression, position: left|center|right }]`, using fixed lists:
  - **Expressions (8):** neutral, happy, sad, angry, surprised, scared, worried, determined.
  - **Poses (6), all standing so they fit any background:** relaxed, arms crossed, confident (hands on hips), gesturing, fighting stance, turned away (looking back).
  - **Poses that involve the scene** (sitting, kneeling, lying down, holding a specific object, touching someone) are composed images, not sprites.
- **Sprites are made lazily per character × outfit × pose × expression,** by editing the base sprite, with a transparent background from the phase 3 model, then cached.
  - A full set would be 48 sprites per outfit, about $3.40 with Gemini. Only the combinations actually used are made; a character usually settles into a few.
  - **Never slow a turn:** if the exact sprite doesn't exist yet, the stage shows the **closest one** that does (the same pose with another expression, or the reverse). The exact sprite is generated after the turn.
  - Swapping only the face on one body per pose is **not** planned. Lining up generated faces on generated bodies is fragile. It comes back only if phase 3 shows full sprites cost too much.
- **Backgrounds** are made per location, empty of people.
- **Rendering:** the stage is drawn in the frontend as layers (background, then sprites at their positions). In the transcript it's a small inline stage card. See and special moments still get composed images.

**Modules:** `images`, `narrator` (stage in `record_changes`), frontend `transcript/`, plus a stage component shared with phase 6.
**Done when:** a Stage-mode session reuses sprites across turns with no new images after the first appearances, and expressions follow the stage directions.
**Rough size:** +350 to +500 production lines.

## Phase 6: VN view

**Goal:** a visual-novel way to read and play the same story.

- **A global toggle,** in the header or settings, switches every story between the transcript and the VN view. The choice is global because VN mode needs nothing special in the stored story. Any story can be read either way, and switching mid-story is safe.
- **VN mode is frontend plus prompt tweaks.** It uses everything story mode has (bookkeeping, the Images setting, composed and stage images). Only the view and a few prompt lines change, applied per turn while the toggle is on:
  - shorter passages that fit a text box
  - more dialogue
  - stage directions on every turn
- **The VN view shows:**
  - the latest composed scene **or** stage, full-screen, kept until the next one
  - the latest reply in a text box over the bottom of it
  - the composer
- A fade when the image changes; no other animation.

**Modules:** frontend only: a new `vn/` feature, reusing the phase 5 stage component.
**Rough size:** +200 to +300 production lines.

## Later

- **World builder:** before play starts, an LLM builds the main cast, places, items and relationships, and optionally generates their assets up front. That's roughly $4 for 8 characters × 6 expressions + 8 locations with Gemini. Three modes per story:
  - **Open:** as now.
  - **Seeded:** built up front, and the narrator may still add.
  - **Closed, with a valve:** the narrator only changes links. Unnamed extras get no card. A truly needed major entity goes through `request_entity`, which the world builder creates in keeping with the world, or answers with an existing entity to reuse.
- **Characters as sub-agents** (a separate feature): the narrator stays director and referee, and asks a present character's agent what it says or does. The agent knows only its persona, goals, secrets and **what it witnessed.** Phases 1–2 already record presence and those fields.
- **A separate bookkeeper pass,** only if phase 2's test set shows the narrator misses too much. It would use the same `record_changes` schema as its structured output.

## Dependencies and total

```
Phase 1 ──► Phase 2 ──► Phase 4 ──► Phase 5 ──► Phase 6
Phase 3 (probe) ────────►┘ (gates phases 4–5)
```

| Phase | Rough production lines |
| --- | --- |
| 1. Typed world model | +700 to +950 |
| 2. Narrator bookkeeping | +350 to +500 |
| 3. Image probe | 0 (script in `docs/probes/`, $3–5 in images) |
| 4. Image engine and asset library | +700 to +850 |
| 5. Stage mode | +350 to +500 |
| 6. VN view | +200 to +300 |
| **Total** | **about +2,300 to +3,100** |

## Risks

| Risk | Guard |
| --- | --- |
| **The narrator skips or botches bookkeeping** | the Scene line, the change line, your overrides, and the test set. A separate bookkeeper can come back with the same schema. |
| **Tool text saved into the story** (Grok) | the phase 2 guard, required before bookkeeping calls become frequent |
| **Faces drift in composed scenes** with 2–4 references | phase 3 measures it. Cap the references, or prefer Stage for those scenes. |
| **Sprites look pasted on** (lighting, halos, no interaction) | a transparency-capable model, and composed images for interaction |
| **Asset costs creep up** | lazy generation (only what a scene needs) and caching. Every asset is recorded as an image cost, so the header shows it. |

## Open decisions

*Decided on 2026-09-30: decisions 7–13.*

1. **Items for extras:** does an unnamed "rusty sword" get an item card, following the same light-card rule, or only named or plot-relevant items? The recommendation is the same rule as characters: a light card if it appears in a scene, a full card if it matters.

## Sources

- [OpenRouter image generation docs](https://openrouter.ai/docs/features/multimodal/image-generation): `input_references` (URLs or base64, limit varies by provider), `aspect_ratio`, `resolution`, `background` (`auto` / `transparent` / `opaque`)
- OpenRouter model list, `https://openrouter.ai/api/v1/models?output_modalities=image` (fetched 2026-09-30). Image-input models include `google/gemini-3.1-flash-image(-preview)`, `x-ai/grok-imagine-image-2.0`, `x-ai/grok-imagine-image-quality`, `openai/gpt-image-2`, `bytedance-seed/seedream-5-0-pro`, `qwen/qwen-image-3` and `black-forest-labs/flux.2-pro`.
- The Grok tool-call leak evidence: `docs/probes/see_variants_results.jsonl`, and `docs/report/turn-costs-finish.md`.
