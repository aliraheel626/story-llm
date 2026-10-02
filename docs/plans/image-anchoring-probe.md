# Image anchoring probe (phase 0)

## Context

[character-image-anchoring.md](../proposals/character-image-anchoring.md) proposes four things:
- a stored **anchor** picture per character
- **sprites** edited from the anchor
- a **background** per place
- three modes that use them: **Full**, **Merge** and **Stage**

All of it assumes things about the image models that nobody has checked:
- that OpenRouter's `/api/v1/images` honours `input_references` and `aspect_ratio`
- that a face survives an outfit edit
- that references keep characters recognisable in a scene
- that a green key-out gives clean sprites

This probe checks them **before any app code is written,** within a **$1.00 budget**. One cheap model runs every step; two others get a short comparison on the same inputs.

| Arm | Model | Calls | Why |
|---|---|---|---|
| **S** | `bytedance-seed/seedream-5-0-flash` | 20 (steps 1–6) | the cheapest candidate that fits: $0.018 per image, references free, up to 14 references, 2:3 and 16:9 |
| **G** | `google/gemini-3.1-flash-image-preview` | 3 (step C) | the app's default; about $0.068 per image |
| **X** | `x-ai/grok-imagine-image-2.0` | 3 (step C) | a minor test: does it work at all, and which quality tier does the app get? $0.04 (low) or $0.06 (medium), plus $0.01 per reference |

Plus one single call:
- **T:** `openai/gpt-image-1-mini` with `background: "transparent"`, to compare true transparency with the key-out.

**Outcome:** a REPORT.md that answers each question below with the images as evidence. The decision on which model to use is made afterwards, by the user, from the report.

## Ground rules

- **No app code changes.** No edits under `src/` or `src-tauri/`. Work on `main`, and commit only the files under `docs/report/image-anchoring-probe/`.
- **Everything goes in `docs/report/image-anchoring-probe/`:** the script (`probe.py`), the key-out code (`keyout.py`), every image, `results.jsonl`, the contact sheets and `REPORT.md`.
- **API key:**
  - The script reads the key the app already uses, `text_model.openrouter.api_key` in `%APPDATA%\com.story-llm.app\secrets.json`, the same way [see_probe.py](../probes/see_probe.py) does.
  - **Read only.** Open the file in read mode, keep the key in memory, and never modify, move, copy or delete `secrets.json`.
  - The key must never appear in any saved file or console output. Before committing, run `git grep -n -i "sk-or" -- docs/report/image-anchoring-probe` and confirm it prints nothing.
- **Spending cap: $1.00.**
  - After every call, add `usage.cost` from the response to a running total kept in `results.jsonl`.
  - Before every call, stop if the total plus $0.10 would pass $1.00.
  - A response with no `usage.cost` counts as $0.10.
  - Run the steps in the order written. If the cap stops the run, the steps not run are listed in the report as "not run: cap".
- **Retries:** at most 1 retry, only for a failed HTTP call (a timeout or a 5xx error). A bad-looking image is a result to record, not a reason to retry.
- **Record everything:**
  - **Every call** gets one line in `results.jsonl`:
    - `step`, `arm`, `model`
    - the request body with each reference replaced by its file name; no key, no base64
    - the HTTP status, and the error message if any
    - `usage.cost`, the wall-clock milliseconds and `media_type`
    - the output file name, its width and height, and whether it has an alpha channel (Pillow `mode`)
  - **Failures too:** a failed call is recorded and marked as one.
- **Judgements are visual, and must be checkable.**
  - Every judged image appears on a contact sheet (a Pillow montage, labelled with step and arm).
  - Each score in REPORT.md names its contact sheet.
  - Use the scale 0 = no, 1 = partly, 2 = yes.
  - Don't edit, crop or filter any image before it's judged.
  - If something couldn't be judged, write "not judged" and say why.
- **Python 3.14 with Pillow 12** (both present). Use only the standard library and Pillow. HTTP goes through `urllib.request`.

## The request

`POST https://openrouter.ai/api/v1/images`, with:
- the header `Authorization: Bearer <key>`
- the body `{model, prompt, aspect_ratio, input_references?, quality?, background?}`

Each reference is sent as `{"type":"image_url","image_url":{"url":"data:<media_type>;base64,<bytes>"}}`.

From the response, take `data[0].b64_json`, `data[0].media_type` and `usage.cost`. Save the image with the extension its `media_type` gives.

**No `quality` and no `resolution`** on arms S, G and X, the same as the app sends today ([openrouter.rs:24-28](../../src-tauri/src/features/images/openrouter.rs#L24-L28)).

**Style prefix:** every prompt starts with the app's default style, `Digital painting, atmospheric scene illustration.`

## The cards (fixed text, the same for every arm)

| Card | Physical features | Outfit |
|---|---|---|
| **Mira** | woman in her thirties, sharp cheekbones, silver-white hair in a long braid, pale grey eyes, a thin scar through her left eyebrow, lean build | **O1:** crimson velvet gown with gold embroidery. **O2:** worn grey travel cloak over brown leathers, tall boots |
| **Kael** | broad-shouldered man in his forties, dark brown skin, close-cropped black beard, shaved head, heavy brow, a burn scar on his right hand | **green** wool coat with brass buttons, black trousers *(green on purpose: it tests the magenta key)* |
| **Inn** (place) | the Gilded Lantern common room: low timber beams, a stone hearth, brass lanterns, long oak tables, rain on leaded windows | — |

## Steps 1–6: arm S only

### 1. Anchors (2 calls)
- **What:** Mira in O1, and Kael.
- **Settings:** `aspect_ratio: "2:3"`, no references.
- **Prompt:** "Full-body character reference: {physical features}. Wearing {outfit}. Standing straight, facing the viewer, arms relaxed, neutral expression. Plain flat light-grey background, no props, no scenery."
- **Judge:**
  - Is it full body?
  - Is the background plain?
  - Does it match the card?

### 2. Outfit edit (1 call)
- **Reference:** the Mira O1 anchor.
- **Prompt:** "This is the same woman. Keep her face, hair, eyes, scar and build exactly as in the reference. Change only her clothes to: {O2}. Same pose, same plain light-grey background." `2:3`.
- **Judge:**
  - Same face?
  - Same hair, and the scar kept?
  - Is the outfit changed as described?

### 3. Background (1 call)
- **Prompt:** "{Inn}. Empty: no people or animals." `16:9`.
- **Judge:**
  - Empty of people?
  - Matches the description?

### 4. Full scenes (5 calls, all `16:9`)

| Id | References, in order | Prompt (after the style and the reference labels) |
|---|---|---|
| F0 | **none** (control) | Mira (her card text, O1) and Kael (his card text) argue across an oak table in the inn, leaning toward each other. |
| F1 | Mira O1 | Mira sits alone by the hearth, firelight on her face. |
| F2 | Mira O2 | Mira walks down a rainy cobbled street at night, cloak pulled tight. |
| F3 | Mira O1, Kael | the same as F0 |
| F4 | Mira O1, Kael, Inn background | the same as F0, set in the room shown in image 3 |

- **Reference labels:** prefix each prompt with "Image 1 is Mira. Image 2 is Kael. Image 3 is the room." (only the labels that apply), then "Keep their faces, hair and clothes exactly as shown in the reference images."
- **F0 vs F3** shows whether references change anything at all. If F3 looks no closer to the anchors than F0, record that `input_references` **appears to be ignored** on that arm.
- **Judge** each image (F1–F4) against the anchors:
  - Is Mira recognisable?
  - Is Kael recognisable? (F3 and F4)
  - Is the outfit right?
  - Does the references' grey background leak in?
  - F4 only: is the inn recognisable from step 3?

### 5. Sprites (8 calls, all `2:3`)
- **Mira:** 6 sprites, all from her O1 anchor (one reference):
  - happy, sad and angry
  - each as **relaxed** and as **arms crossed**
  - on flat green `#00FF00`
- **Kael:** 1 sprite (neutral, relaxed) on **magenta** `#FF00FF`, and the same one on **green**. The green one is there to show what green-on-green does.
- **Prompt:** "The same character as the reference: keep face, hair, build and clothes exactly. Pose: {pose}. Expression: {expression}. Full body. Background: a perfectly flat, uniform {colour name} ({hex}) with no shadows, gradients, floor or props."
- **Judge:**
  - Is it the same character across the 6 Mira sprites?
  - Do the expression and pose match?
  - Is the background flat?

### 6. Merge (3 calls, all `16:9`, 3 references each)
- **References for M1 and M2:** Inn background, Mira (sad, arms crossed), Kael (neutral, relaxed). Use the **unkeyed** sprites.

| Id | Prompt |
|---|---|
| M1 | Image 1 is the room. Place image 2 (Mira) on the left and image 3 (Kael) on the right, standing, in the room's lighting. Remove the sprites' solid backgrounds. |
| M2 | The same, with Kael on the left and Mira on the right. |
| M3 | The same references as M1, but Mira (happy, relaxed) instead of sad. |

- **Judge:**
  - Is the room recognisable?
  - Are both characters recognisable?
  - Do the left and right positions match the prompt?
  - Does any green or magenta remain?
  - Is the lighting blended (not pasted on)?

## 7. Key-out (no API calls; `keyout.py` on every green and magenta sprite)
- **Border check:** take the share of pixels in the outer 2% border within the key-colour tolerance. Record it per sprite. The proposal's rule is ≥ 90%. Report whether every flat-looking sprite passes and whether any non-flat one would pass.
- **Key-out:**
  1. Flood fill from all four edges, removing pixels within tolerance of the key colour. Default: Euclidean RGB distance ≤ 90; record the value used.
  2. Despill: on pixels next to removed ones, cap the key channel at the mean of the other two (for green, `g = min(g, (r+b)/2)`; for magenta, the mirror).
  3. Give the outermost remaining ring 50% alpha.
- **Output per sprite:**
  - the keyed PNG
  - a composite of it over the Inn background, in the centre, scaled to 80% of the height
- **Judge:**
  - Are the hair edges clean?
  - Is any of the figure removed? Check Kael's green coat on green, and on magenta.
  - Is there a halo?

## 8. True transparency (T, 1 call)
- `openai/gpt-image-1-mini` with `background: "transparent"`, `quality: "medium"`, `2:3`.
- **Reference:** arm S's Mira O1 anchor, with the step 5 prompt for happy and relaxed, the background line removed.
- Composite it over arm S's Inn exactly as in step 7, and run the step 7 border check in its "transparent" form (≥ 90% of the border pixels at alpha 0).

## C. Comparison (arms G and X, 3 calls each)

Both arms get **the same requests**, built on **arm S's** pictures, so all three models are judged against the same anchors. Run G's three calls, then X's.

| Id | References, in order | Prompt | Settings |
|---|---|---|---|
| C1 | S's Mira O1 anchor | the step 2 outfit edit | `2:3` |
| C2 | **none** | the step 4 F0 prompt | `16:9` |
| C3 | S's Mira O1 anchor, S's Kael anchor | the step 4 F3 prompt, with its reference labels | `16:9` |

- **Grok's tier:** the app sends no `quality`, and neither does this step. Grok bills $0.04 (low) or $0.06 (medium) per image plus $0.01 per reference, so C2's cost alone gives the tier, and C1 and C3 should agree with it. Record the tier. Anything else, record it as it is.
- **Judge** each arm's C1–C3 against S's anchors, with the same questions as steps 2 and 4. Put S's own step 2, F0 and F3 next to them on one contact sheet, so the three models can be compared side by side.
- **Grok works?** Report plainly whether each call succeeded, whether `aspect_ratio` was honoured and whether C3 looks closer to the anchors than C2.

## Expected spend

| Part | Calls | Estimate |
|---|---|---|
| Arm S (steps 1–6) | 20, 25 references (free) | about $0.36 |
| T | 1 | about $0.01–0.02 |
| Arm G (step C) | 3, 3 references | about $0.20–0.25 |
| Arm X (step C) | 3, 3 references | $0.15–0.21, depending on the tier |
| **Total** | **27** | **about $0.72–0.84** (cap $1.00) |

## REPORT.md

`docs/report/image-anchoring-probe/REPORT.md`, with these sections. Every number comes from `results.jsonl`, and every score names its contact sheet.

1. **Spend and speed:** cost and average and maximum milliseconds, per arm and per step, plus the total spend.
2. **API behaviour, per arm:**
   - Was `aspect_ratio` honoured? Give the actual width × height against 2:3 and 16:9.
   - Were references honoured (F0 vs F3 on S, C2 vs C3 on G and X)?
   - Any errors.
3. **Grok:** did it work, and the tier its costs imply.
4. **Identity table:** the scores for steps 1, 2, 4, 5 and 6 on S, and the step C scores for S, G and X side by side.
5. **Background leak:** steps 4, 6 and C.
6. **Key-out:**
   - each sprite's border share
   - whether the 90% rule separated flat from non-flat
   - the tolerance used
   - the step 7 judgements, including Kael's green coat on green against magenta
7. **True transparency:** T against the key-out.
8. **Answers to the proposal's questions,** one line each, each citing its evidence:
   - Do Full scenes keep identity on S? On G and X (C3)?
   - Does an outfit edit keep the face, on each arm?
   - Does Merge work with 3 references on S?
   - Is the colour key good enough as the fallback?
   - Is the 90% border threshold right, or what should it be?
9. **Anything unexpected,** and anything not done, with the reason.

Don't write a recommendation for which model to adopt. The user decides that from this report.

## Verification (by the reviewer)

- **Spend:** the sum of `usage.cost` in `results.jsonl` matches the report, and is within $1.00.
- **Evidence:**
  - every image named in `results.jsonl` exists
  - every image is on a contact sheet
  - every score cites a sheet
- **References:** each reference sent was the file the step names. Check the logged reference names against the step tables; step C must reference arm S's files.
- **No secrets:** `git grep -n -i "sk-or"` under the report folder prints nothing, and `secrets.json` is unchanged (same size and modified time as before the run).
- **App untouched:** `git status` shows no change outside `docs/report/image-anchoring-probe/`.
