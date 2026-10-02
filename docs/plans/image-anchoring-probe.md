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

This probe checks them **before any app code is written,** on two models side by side:

| Arm | Model | Why |
|---|---|---|
| **G** | `google/gemini-3.1-flash-image-preview` | the app's default image model |
| **X** | `x-ai/grok-imagine-image-2.0` | the alternative; up to 3 references, $0.01 per reference, $0.04 (low) or $0.06 (medium) per 1K image |

Plus a few single calls:
- **T:** `openai/gpt-image-1-mini` with `background: "transparent"`, to compare true transparency with the key-out.
- **R:** `sourceful/riverflow-v2.5-fast`. It lists `background: transparent`, but its only output format is JPEG, which can't hold transparency. One call shows what it actually returns.
- **Q:** one Grok call with no `quality`, to find which tier the app gets today. The app never sends `quality` ([openrouter.rs:24-28](../../src-tauri/src/features/images/openrouter.rs#L24-L28)).

**Outcome:** a REPORT.md that answers each question below with the images as evidence. The decision on which model to use is made afterwards, by the user, from the report.

## Ground rules

- **No app code changes.** No edits under `src/` or `src-tauri/`. Work on `main`, and commit only the files under `docs/report/image-anchoring-probe/`.
- **Everything goes in `docs/report/image-anchoring-probe/`:** the script (`probe.py`), the key-out code (`keyout.py`), every image, `results.jsonl`, the contact sheets and `REPORT.md`.
- **API key:**
  - The script reads it from the `OPENROUTER_API_KEY` environment variable, which the user sets.
  - **Never open, read, copy or print `secrets.json`.** If the variable is missing, stop and ask the user.
  - The key must never appear in any saved file or console output. Before committing, run `git grep -n -i "sk-or" -- docs/report/image-anchoring-probe` and confirm it prints nothing.
- **Spending cap: $5.00.**
  - After every call, add `usage.cost` from the response to a running total kept in `results.jsonl`.
  - Before every call, stop if the total plus $0.10 would pass $5.00.
  - A response with no `usage.cost` counts as $0.10.
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

**Fixed per arm:**
- **G:** no `quality`.
- **X:** `quality` set explicitly to the tier step Q finds. That's what the app gets today. If Q is inconclusive, use `"low"` and say so.

**Style prefix:** every prompt starts with the app's default style, `Digital painting, atmospheric scene illustration.`

## The cards (fixed text, the same for both arms)

| Card | Physical features | Outfit |
|---|---|---|
| **Mira** | woman in her thirties, sharp cheekbones, silver-white hair in a long braid, pale grey eyes, a thin scar through her left eyebrow, lean build | **O1:** crimson velvet gown with gold embroidery. **O2:** worn grey travel cloak over brown leathers, tall boots |
| **Kael** | broad-shouldered man in his forties, dark brown skin, close-cropped black beard, shaved head, heavy brow, a burn scar on his right hand | **green** wool coat with brass buttons, black trousers *(green on purpose: it tests the magenta key)* |
| **Inn** (place) | the Gilded Lantern common room: low timber beams, a stone hearth, brass lanterns, long oak tables, rain on leaded windows | — |

## Steps (run each step for arm G, then arm X, unless marked otherwise)

### Q. Grok's default quality (arm X only, run first)
- **Call:** Kael's anchor prompt (step 1) on Grok, **without** `quality`.
- **Read:** `usage.cost`. **$0.04** means low and **$0.06** means medium. Anything else, record it as it is.
- **Keep the image:** it's also arm X's Kael anchor, so step 1 for Kael in arm X is skipped.

### 1. Anchors (2 calls per arm)
- **What:** Mira in O1, and Kael.
- **Settings:** `aspect_ratio: "2:3"`, no references.
- **Prompt:** "Full-body character reference: {physical features}. Wearing {outfit}. Standing straight, facing the viewer, arms relaxed, neutral expression. Plain flat light-grey background, no props, no scenery."
- **Judge:**
  - Is it full body?
  - Is the background plain?
  - Does it match the card?

### 2. Outfit edit (1 call per arm)
- **Reference:** the arm's Mira O1 anchor.
- **Prompt:** "This is the same woman. Keep her face, hair, eyes, scar and build exactly as in the reference. Change only her clothes to: {O2}. Same pose, same plain light-grey background." `2:3`.
- **Judge:**
  - Same face?
  - Same hair, and the scar kept?
  - Is the outfit changed as described?

### 3. Background (1 call per arm)
- **Prompt:** "{Inn}. Empty: no people or animals." `16:9`.
- **Judge:**
  - Empty of people?
  - Matches the description?

### 4. Full scenes (5 calls per arm, all `16:9`)

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

### 5. Sprites (8 calls per arm, all `2:3`)
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

### 6. Merge (3 calls per arm, all `16:9`; uses 3 references, which is Grok's limit)
- **References for M1 and M2:** Inn background, Mira (sad, arms crossed), Kael (neutral, relaxed). Use the arm's own **unkeyed** sprites.

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

### 7. Key-out (no API calls; `keyout.py` on every green and magenta sprite)
- **Border check:** take the share of pixels in the outer 2% border within the key-colour tolerance. Record it per sprite. The proposal's rule is ≥ 90%. Report whether every flat-looking sprite passes and whether any non-flat one would pass.
- **Key-out:**
  1. Flood fill from all four edges, removing pixels within tolerance of the key colour. Default: Euclidean RGB distance ≤ 90; record the value used.
  2. Despill: on pixels next to removed ones, cap the key channel at the mean of the other two (for green, `g = min(g, (r+b)/2)`; for magenta, the mirror).
  3. Give the outermost remaining ring 50% alpha.
- **Output per sprite:**
  - the keyed PNG
  - a composite of it over the arm's Inn background, in the centre, scaled to 80% of the height
- **Judge:**
  - Are the hair edges clean?
  - Is any of the figure removed? Check Kael's green coat on green, and on magenta.
  - Is there a halo?

### 8. True transparency (single calls)
- **T:** `openai/gpt-image-1-mini` with `background: "transparent"`, `quality: "medium"`, `2:3`. The reference is arm G's Mira O1 anchor, with the step 5 prompt for happy and relaxed, the background line removed. Composite it over arm G's Inn exactly as in step 7.
- **R:** `sourceful/riverflow-v2.5-fast`, the same request. Record its `media_type`, the Pillow mode, and whether any pixel is transparent. Run the step 7 border check in its "transparent" form (≥ 90% of the border pixels at alpha 0).

## Expected spend

| Part | Calls | Estimate |
|---|---|---|
| Arm G | 20 (steps 1–6), 25 references | about $1.40 |
| Arm X | 20 (Q replaces Kael's anchor), 25 references | $1.05–1.45, depending on the tier Q finds |
| T + R | 2 | under $0.10 |
| **Total** | **42** | **about $2.50–2.90** (cap $5.00) |

## REPORT.md

`docs/report/image-anchoring-probe/REPORT.md`, with these sections. Every number comes from `results.jsonl`, and every score names its contact sheet.

1. **Spend and speed:** cost and average and maximum milliseconds, per arm and per step, plus the total spend.
2. **Grok's default tier:** the Q cost and the tier it implies.
3. **API behaviour, per arm:**
   - Was `aspect_ratio` honoured? Give the actual width × height against 2:3 and 16:9.
   - Were references honoured (F0 vs F3)?
   - Any errors.
4. **Identity table:** the scores for steps 1, 2, 4, 5 and 6, side by side for G and X.
5. **Background leak:** steps 4 and 6.
6. **Key-out:**
   - each sprite's border share
   - whether the 90% rule separated flat from non-flat
   - the tolerance used
   - the step 7 judgements, including Kael's green coat on green against magenta
7. **True transparency:** T against the key-out; what R actually returned.
8. **Answers to the proposal's questions,** one line each, each citing its evidence:
   - Do Full scenes keep identity on G? On X?
   - Does an outfit edit keep the face?
   - Does Merge work within Grok's 3-reference limit?
   - Is the colour key good enough as the fallback?
   - Is the 90% border threshold right, or what should it be?
9. **Anything unexpected,** and anything not done, with the reason.

Don't write a recommendation for which model to adopt. The user decides that from this report.

## Verification (by the reviewer)

- **Spend:** the sum of `usage.cost` in `results.jsonl` matches the report, and is within $5.00.
- **Evidence:**
  - every image named in `results.jsonl` exists
  - every image is on a contact sheet
  - every score cites a sheet
- **References:** each reference sent was the file the step names. Check the logged reference names against the step table.
- **No secrets:** `git grep -n -i "sk-or\|secrets.json"` under the report folder prints only this plan's rule text, if anything.
- **App untouched:** `git status` shows no change outside `docs/report/image-anchoring-probe/`.
