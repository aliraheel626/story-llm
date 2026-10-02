# Character Image Anchoring And The Three Image Modes: Proposal

A design proposal, 2026-10-03, checked against `main` (`2cb3e0b`) and OpenRouter's live image model list. It replaces phases 3–5 of [vn-mode-and-consistent-images.md](vn-mode-and-consistent-images.md) for images only. Nothing has been built, and no image has been generated for it. Claims about how well the models keep a character consistent stay untested until phase 0.

**Why:** anchors, sprites, backgrounds and a layered stage are what a VN mode would later be built from. **This proposal is limited to the image side.** The VN view, the typed world model and `record_changes` are out of scope.

## What it adds

1. **Physical features and outfit as separate card fields,** each with a strict meaning, because an anchor is remade differently depending on which one changed.
2. **Character anchors.** Each character gets a stored reference picture, made from their card's physical features and outfit. Every image of that character is made from, or with, that picture, so the face and clothes stay the same from scene to scene.
3. **Three image modes,** set per story:

   | Mode | How a scene image is made | Image calls per scene once assets exist |
   | --- | --- | --- |
   | **Full** | The image model draws the whole scene, with the anchors of the characters in it as references. | 1 |
   | **Merge** | The app gathers a background and one sprite per character (the right pose and expression), and the image model merges them into one scene. | 1 |
   | **Stage** | No image model call. The app layers transparent sprites over the background in code. | 0 |

4. **Reuse earlier images,** an on/off option next to the mode. Before making a new scene image, the app looks for a stored one made for the same situation and shows that instead, at no cost.

## Where things stand today

| Piece | Finding |
| --- | --- |
| The image request | Sends only `model` + `prompt` ([openrouter.rs:24-28](../../src-tauri/src/features/images/openrouter.rs#L24-L28)). The API also takes `input_references` (base64 data URLs accepted), `aspect_ratio` and `background`. |
| Reference limits | **Every model reports its own limit** in `GET /api/v1/images/models` (`supported_parameters.input_references.max`). Gemini 3.1 Flash Image (current default, preview and non-preview): 14. GPT Image: 16. FLUX.2: 8. Grok Imagine: 3. The app can read the limit instead of hard-coding it. |
| Transparency | Only the OpenAI GPT Image models (`gpt-image-1`, `-1-mini`, `2.5-flare`/`-sunburst`, `gpt-5-image(-mini)`) and Sourceful Riverflow 2.5 list `background: transparent`. **Gemini does not,** so the current model can't make Stage sprites directly. |
| Consistency today | Text only: `compose_image_prompt` appends each named character's `appearance_anchor` ([generation.rs:34](../../src-tauri/src/features/images/generation.rs#L34)). `characters_by_ids` drops characters with no `appearance_anchor` and ignores `outfit` ([generation.rs:45](../../src-tauri/src/features/images/generation.rs#L45)). |
| `illustrate_scene` | `{description, characters?: [name]}` ([illustrate_scene.rs](../../src-tauri/src/features/narrator/tools/illustrate_scene.rs)). One image per turn. See forces the call in code. There is no location or expression argument. |
| Character cards | `characters` has `known_as`, `appearance_anchor`, `gender`, `age`, `role`, `location`, `outfit`, all free text ([db.rs:124](../../src-tauri/src/shared/db.rs#L124)). There are **no location records:** `location` is a phrase on each character. |
| Image storage | `image_assets` + `image_blobs` in SQLite, and every asset must belong to a transcript entry (`entry_id NOT NULL`, [db.rs:173](../../src-tauri/src/shared/db.rs#L173)). Served to the webview through the `storyimg://` scheme ([lib.rs:30](../../src-tauri/src/lib.rs#L30)). The transcript lists images by `image_assets.entry_id` ([attachments.rs:16](../../src-tauri/src/features/transcript/attachments.rs#L16)), so one picture can't show under two entries. |
| Timing | Images are generated **inside the turn's transaction,** after the narration ([generation.rs:253](../../src-tauri/src/features/images/generation.rs#L253)). The turn finishes when its image does. |
| Cost and speed | The 12 stored images are all Gemini 3.1 Flash Image Preview: **$0.068 and 10.6 s on average.** |
| On/off | The global "enabled" switch in the Image Model panel, plus the story's "Illustrate scenes" narrator tool ([NarratorToolsPanel.tsx:22](../../src/features/narratorTools/NarratorToolsPanel.tsx#L22)). See works whenever images are enabled. Both stay as they are. |

## Decisions carried over

From the earlier proposal, still in force unless you change them:

- **See always makes a new Full image,** whatever the mode and the reuse option.
- **Scenes are 16:9.**
- **Assets are made lazily,** only when a scene needs them. A stale asset is remade the next time it's used, not all at once.
- **Sprites use 8 expressions** (neutral, happy, sad, angry, surprised, scared, worried, determined) **and 6 standing poses** (relaxed, arms crossed, hands on hips, gesturing, fighting stance, turned away). Poses that involve the scene, such as sitting, kneeling or touching someone, can't be sprites. They're Full images.
- **Waiting for the image is fine.** Images stay part of the turn, so any new assets are made inside the turn as well.

## Card fields: physical features and outfit

The card already has two fields, `appearance_anchor` ("Appearance") and `outfit`. **In practice they blur.** In your database, **8 of 25 looks mention clothing,** and some include pose or mood. Some outfits include things that aren't clothes. For example (read-only):

| Character | Look (`appearance_anchor`) | Outfit |
| --- | --- | --- |
| Mira | "**wearing a worn grey cloak**, face set with old resentment, **watching the door**" | "father's grey cloak" |
| Hedda Grane | "broad-shouldered woman … hair pinned back, **leather apron and a brass key on a chain**" | "black oilskin cloak, **forearm bound in cloth**, brass key on a chain" |
| Wren Calder | "… **still hands around an untouched cup**" | "dark hooded cloak, **untouched drink before her**" |

Anchors depend on the split. A change of clothes edits the existing anchor, so the face stays. A change of body makes a new anchor. With clothing inside the look, every outfit change also looks like a body change.

- **`appearance_anchor` is renamed to `physical_features`** (UI: "Physical features"). It holds the body only: face, hair, eyes, skin, build, height, scars and tattoos. **Never** clothing, worn items, pose, mood or what they're doing.
- **`outfit` stays** (UI: "Wearing now"). It holds clothing and anything worn or carried on the body: a cloak, an apron, a key on a chain, a sword on the hip. **Never** the body, the pose, or objects nearby.
- **Gender and age** stay their own fields and count as physical features for anchors.
- **Enforced through the field descriptions,** in `save_character`'s schema and in the card's placeholder text. There's no code check: a word list can't tell "a scarred lip" from "a scarf". Phase 1's QA reads the saved fields from 10 turns and counts mix-ups.
- **Your existing cards:** the rename comes with the by-hand schema step (delete the database). No data is converted.

## The assets

| Asset | What it is | Keyed by | Made from | Used by |
| --- | --- | --- | --- | --- |
| **Anchor** | The character standing in a neutral pose, full body, on a plain background, 2:3 | character + **body version** (a hash of physical features, gender, age and the style text) + **outfit** (the outfit text) | a new body version: from text. A new outfit on the same body: **an edit of that body's latest anchor,** so the face carries over | all three modes |
| **Sprite** | The anchor in one pose with one expression; transparent for Stage | anchor version + pose + expression | an edit of the anchor | Merge, Stage |
| **Background** | A place with no people in it, 16:9 | location name (trimmed, case-insensitive) | text | Merge, Stage (and Full, as an optional extra reference) |
| **Scene** | The picture shown in the transcript | its transcript entry, plus **tags** for reuse | the mode's recipe | the transcript, Reuse |

- **Versions instead of overwriting.** When a card's physical features or outfit change, the old anchor is kept and a new one is made the next time a scene needs it. If the outfit changes back, the old anchor is current again at no cost. In effect each body version keeps a wardrobe.
- **Assets are a cache, not story events.** Retry and Erase don't remove anchors, sprites or backgrounds. They go only when the character or the story is deleted. Scenes stay tied to their entry, as now.
- **Your pictures win.** An anchor you upload or pick is **pinned.** An outfit change makes a new anchor by editing your picture. A physical-features change doesn't replace it: the card marks it "stale", and you choose to keep it or regenerate.
- **Without physical features there's no anchor.** A character with no physical features and no outfit text gets no anchor. Their image falls back to text, as now.

### Schema (applied by hand: delete the database, keep `secrets.json`)

`characters.appearance_anchor` is renamed `physical_features`.

`image_assets` gains:
- `kind`: `scene` | `anchor` | `sprite` | `background`
- nullable `entry_id`: set for scenes only
- nullable `entity_id`: references `entities`
- `variant_key`: `body/outfit` for an anchor, `body/outfit/pose/expression` for a sprite, or the location key
- `pinned`
- `tags_json`: scenes only, for Reuse

A scene's `image_generated` entry already carries `asset_id`. The transcript then lists images **from those entries** instead of from `image_assets.entry_id`, so a reused picture can show under a later entry.

## The tool change

`illustrate_scene` takes one schema in every mode. Fields the mode doesn't need are ignored.

```
illustrate_scene({
  description: "...",                       // as now
  location?:   "the Rusty Anchor",          // names a background; reuse an existing name
  characters?: [{ name, expression?, pose?, position?: left|center|right }]
})
```

- **Names and fixed lists.** `characters` changes from names to objects. `expression` and `pose` are enums from the fixed lists.
- **`characters` left out:** the code matches present characters' names and `known_as` in the description, as the old See heuristic did. Today an empty list means no anchors at all.
- **`location` left out:** it defaults to the last scene's location.
- **Known background names.** In Merge and Stage, `<entities>` gains one line, "Backgrounds: the Rusty Anchor, the docks", so the narrator reuses names instead of inventing "the Rusty Anchor tavern".

## How each mode makes a scene

All modes first check **Reuse** (below), then make any missing assets **in parallel** before the scene, so two new characters cost one wait instead of two. **New assets are capped at 3 per scene.** Past the cap, a character falls back as described in each mode, and the missing asset is made next time.

### Full
- **References:** the anchors of the characters in the scene, in the order given, up to the model's limit (read from the model list) and a cap of 4. The location's background is added if one already exists. Full never makes a background.
- **Prompt:** labelled, for example "Image 1 is Mira. Image 2 is Kael. Keep their faces, hair and clothes exactly as shown." Then the style, the description and the existing text block.
- **Fallback:** a character without an anchor is described in text only, as today.
- **Cost:** 1 image, plus the first anchor for each new character.

### Merge
- **Assets:** the background for `location` and one sprite per character for their pose and expression. Missing ones are made first.
- **Prompt:** "Image 1 is the background. Place image 2 (Mira) on the left and image 3 (Kael) on the right, in the background's lighting. {description}." The model blends edges, lighting and interaction, which Stage can't.
- **Fallback:** without a sprite, the anchor is used. Without a background, the scene falls back to Full.
- **Cost:** 1 image per scene. The first scene in a new place with two new characters can mean up to 4 asset images.

### Stage
- **No image call for the scene.** The transcript gets a **stage entry** (`stage_shown`) listing the background and each sprite with its position. The frontend draws them as layers in a 16:9 card.
- **Sprites need real transparency.** The sprite model is chosen by what it reports in the model list:
  - **If it lists `background: transparent`** (GPT Image, Riverflow 2.5): the request asks for it, and the PNG comes back with alpha.
  - **Otherwise (the fallback, e.g. Gemini): a colour key.** The sprite is drawn on a flat key colour, which the code removes with the `image` crate (already in `Cargo.lock` through Tauri, 0.25). The removal has three guards:
    - **Key colour per character:** green, unless the character's outfit or physical features name green, in which case magenta.
    - **Flood fill from the edges:** only key-coloured pixels connected to the border are removed, so key-coloured details inside the figure survive.
    - **Despill and a soft edge:** key colour mixed into the hair outline is pulled out, and the outermost pixel ring gets partial alpha instead of a hard cut.
  - **Border check (both routes, no model call):** before a sprite is stored, the code reads its border pixels.
    - **Colour key:** at least 90% of the border must be close to the key colour. This catches a generator that drew a scene, a grey backdrop or a gradient instead of flat green.
    - **Native transparency:** at least 90% of the border must be transparent.
    - **On failure:** the sprite is generated once more. If that also fails, the character falls back to their closest existing sprite or their anchor for this turn, and the failure is logged. A failed attempt still counts as an image cost.
    - **Limit:** it can't catch extra things drawn onto an otherwise flat background. That would need a vision check, which is left out on cost.
- **Sprite model setting:** the sprite model defaults to the scene model. You can pick a separate one in the Image Model panel, which only matters if you want true transparency while keeping Gemini for scenes.
- **Closest-sprite fallback:** a missing sprite is made in the turn if under the cap. Otherwise the stage shows the closest existing one: same pose with another expression, then the anchor.
- **No captions for stage entries.** The narrator already knows exactly what was shown, because the layers come from its own directions.
- **Cost:** $0 once a character's sprites exist.

## Reuse earlier images

- **Where it applies:** Full and Merge, on narrator-initiated images. See never reuses. Stage always reuses its layers, so the option doesn't apply there.
- **Scene tags:** each new scene stores its location key, plus each character's anchor version, pose and expression, in `tags_json`.
- **Exact match only:** a reuse needs the same location, the same set of characters, the same anchors (physical features and outfits), and the same expression and pose for each. So "Mira, sad, in the throne room, in the crimson gown" brings back that picture. "Mira, happy" or "Mira in travel clothes" makes a new one.
- **How a reuse shows:** an `image_generated` entry pointing to the old `asset_id`, with `reused: true`, at no cost. The UI marks it "reused".
- **Picking from matches:** if several match, the newest is used.

## Card and settings UI

- **Narrator Tools panel,** under "Illustrate scenes": a **Mode** choice (Full / Merge / Stage) and **"Reuse earlier images when they fit"**. Stored in the story's `settings_json`. New stories default to Full, with reuse off.
- **Character card:**
  - the current anchor as a thumbnail, with a "stale" mark when the card has changed since it was made
  - Generate / Regenerate / Upload / Pin
  - the wardrobe: earlier anchor versions, any one of which can be picked as current
  - the sprites made so far, in a small grid
- **Backgrounds:** a short list in the entities panel, with Regenerate and Upload.
- **Image Model panel:** the reference limit read from the model list, shown read-only, and, if phase 0 says so, a **sprite model**.
- **Costs:** every asset is recorded as an image cost on its turn, so the per-turn and header totals include it.

## Phases

### Phase 0: Probe (a script, no app code; plan: [image-anchoring-probe.md](../plans/image-anchoring-probe.md))

**Goal:** prove the image API does what the modes assume, on the current model, before building anything.

- **Hand-made cards:** 2 characters, one with two outfits, and 1 location.
- **Tests:**
  1. **Anchor:** text to anchor. Then an outfit edit of that anchor: is it the same face?
  2. **Full:** 4 scenes with 1–2 anchors as references. Does identity hold, does the outfit hold, and do the references' plain backgrounds leak into the scene?
  3. **Merge:** background + 2 sprites → 3 scenes. Is the background recognisable? Are the characters placed as asked?
  4. **Sprites:** 3 expressions × 2 poses edited from the anchor. Are they consistent?
  5. **Stage transparency:** a transparent sprite from one GPT Image model, against the colour-key fallback (green, and magenta for a green-clad character), run through the key-out code. How are the edges around hair, and does any clothing get removed? Record each sprite's border-match share to confirm the 90% threshold.
  6. **API check:** does `/api/v1/images` actually honour `input_references` and `aspect_ratio: 16:9`? Use a read-only check of the output size and a side-by-side image with and without the references.
- **Models (budget $1):**
  - Seedream 5.0 Flash ($0.018 per image, references free) runs every test.
  - Gemini (the current default) and Grok Imagine 2.0 each get 3 comparison calls on Seedream's anchors: an outfit edit, and a scene with and without references. Grok's calls also show its default quality tier.
  - One true-transparency call (GPT Image 1 mini).
- **Output:** 27 calls, about **$0.72–0.84** (cap $1), saved with a REPORT.md in `docs/report/image-anchoring-probe/`.

**Done when:** the report says, with the images as evidence, whether Full and Merge keep identity on Seedream, how Gemini and Grok compare on the same anchors, and whether the colour-key fallback is good enough to be the default for models without transparency.

### Phase 1: Anchors and Full mode

- **Card fields:** rename `appearance_anchor` to `physical_features` everywhere it appears (14 files: schema, models, events, the tool, the `<entities>` line, the image prompt, the card UI). Write the strict descriptions for both fields.
- **Request:** `input_references` (base64 data URLs from `image_blobs`), `aspect_ratio` and, when asked, `background` in `openrouter.rs`. Read each model's limits from the model list, cached per session.
- **Storage:** the schema change above. An anchors module: look up the current version, make it when missing (text, or an edit of the previous version), and enforce the cap and parallel generation.
- **Tool and generation:** `illustrate_scene` takes the new schema, with the name fallback, and Full's labelled prompt and references.
- **UI:** anchor thumbnail, Generate, Regenerate, Upload, Pin and the wardrobe on the card. The Mode setting, with only Full enabled.

**Done when:** in the running app:
- A character introduced with physical features gets an anchor on their first scene.
- Their next two scenes use it as a reference, checked by reading the request's reference count from a debug log.
- An outfit change makes a new anchor on the next scene by editing the previous one. Changing the outfit back reuses the old one with no image call.
- Across 10 narrator turns, at most 1 saved field mixes clothing into physical features or the body into the outfit.

**Rough size:** +370 to +520 production lines (backend about +260, frontend about +160). The rename itself is mostly line-for-line.

### Phase 2: Reuse earlier images
- **Backend:** `tags_json` on scenes, the exact-match lookup, reused entries, and the transcript listing images from entries.
- **UI:** the option and the "reused" mark.

**Done when:** in the running app, returning to the same place with the same characters, outfits and expressions shows the earlier picture with no image usage row. Changing one expression makes a new one.

**Rough size:** +150 to +220.

### Phase 3: Backgrounds, sprites and Merge mode
- **Assets:** backgrounds and sprites (edits of the anchor), and the backgrounds line in `<entities>`.
- **Merge:** the recipe and its fallbacks. Reuse also covers Merge scenes.
- **UI:** the sprite grid on the card and the backgrounds list.

**Done when:** in the running app, a second scene in the same place reuses the stored background and sprites as references, with one image call. The phase 0 placement checks hold.

**Rough size:** +300 to +400.

### Phase 4: Stage mode
- **Backend:** native transparency when the sprite model lists it, the colour-key fallback otherwise, the border check with one retry, the sprite model setting, the `stage_shown` entry and the closest-sprite fallback.
- **Frontend:** a stage card component. This is the piece a later VN view would reuse.

**Done when:**
- In the running app, a Stage session makes no image calls after the first appearances.
- The expressions and positions on screen follow the narrator's directions, checked against the `stage_shown` payloads.
- A unit test feeds the border check a flat-key image, a scenic image and a transparent PNG, and gets pass, fail, pass.

**Rough size:** +260 to +370.

| Phase | Rough production lines | Image spend |
| --- | --- | --- |
| 0. Probe | 0 (script) | $0.72–0.84 (cap $1) |
| 1. Fields, anchors and Full | +370 to +520 | QA only |
| 2. Reuse | +150 to +220 | QA only |
| 3. Merge | +300 to +400 | QA only |
| 4. Stage | +260 to +370 | QA only |
| **Total** | **about +1,080 to +1,510** | |

```
Phase 0 ──► Phase 1 ──► Phase 2
                  └───► Phase 3 ──► Phase 4
```

## Risks

| Risk | Guard |
| --- | --- |
| **Gemini ignores or weakly follows references** | Phase 0 checks it before any code. Every listed model reports its limit, so switching models is a setting, not a rewrite. |
| **A reference's plain background leaks into Full scenes** | Phase 0 measures it. If it does, the prompt names the background as "ignore", or Full also passes the location's background. |
| **Slow first scenes:** a new place with two new characters in Merge is up to 4 assets + 1 scene | Assets are made in parallel (about 2 waits, not 5), with the 3-asset cap. Later scenes cost one image or none. |
| **Location names drift** ("the Rusty Anchor" vs "the Rusty Anchor tavern") | The backgrounds line in `<entities>`, case-insensitive keys, and renaming or merging a background in the list. Real location cards remain a later option. |
| **Replay deletes and recreates entity rows** ([projection.rs:211](../../src-tauri/src/features/entities/projection.rs#L211)), which could cascade-delete a character's anchors on Retry | The phase 1 plan checks whether Retry recreates a character under the same id. If not, assets reference the entity without a cascade and are removed by the delete command. |
| **The generator ignores the flat background** (draws a scene or a gradient) | The border check with one retry, then the closest existing sprite. |
| **Stage sprites look pasted on** (no shared lighting, halos around hair) | The phase 0 transparency test, and a separate transparent-capable sprite model if the key-out falls short. Scenes that need interaction are Merge, Full or See. |
| **Asset costs creep up** | Lazy generation, the per-scene cap, versions instead of remakes, and every asset recorded as a cost on its turn. |

## Decisions (settled 2026-10-03)

1. **Stage sprites:** native transparency when the model supports it, with a colour key as the fallback for models that don't, guarded by the border check.
2. **Reuse match:** exact. The place, the characters, their physical features and outfits, and each character's expression and pose must all match.
3. **Anchor framing:** full body, standing, 2:3. Phase 0 still checks how well the face holds in Full scenes.
4. **Pinned anchors:** your picture stays the body and face until you replace it.
   - **Outfit change:** a new anchor is made by editing your picture.
   - **Physical-features change:** your picture is kept, and the card marks it "stale" for you to keep or regenerate.
5. **Asset cap:** 3 new assets per scene. Past the cap, the scene falls back, and the missing assets are made next time.

## Sources

- [OpenRouter image generation docs](https://openrouter.ai/docs/features/multimodal/image-generation), fetched 2026-10-03:
  - `input_references`: an array of `{type: "image_url", image_url: {url}}`, taking HTTP(S) or base64 data URLs, with a per-provider limit
  - `aspect_ratio`, including `16:9`
  - `background`: `auto` / `transparent` / `opaque`
  - `seed`
- `GET https://openrouter.ai/api/v1/images/models`, fetched 2026-10-03: 57 models, each with `supported_parameters.input_references` and, where supported, `background`.
- Local database (read-only): 12 image usage rows, all `google/gemini-3.1-flash-image-preview`, averaging $0.0683 and 10.6 s each.
