# Characters and relationships: a character-only entity model, one tool each

## Context

**The current code (`main` at `71ef4b8`):** an entity has a free-text `kind`. The narrator tool descriptions offer five kinds (`character`, `object`, `location`, `relationship`, `campaign`), and every entity has the same two fields, name and appearance (`entities` plus `story_entity_state`). Only characters have a UI panel. A relationship entity has no endpoints, and nothing reads `campaign`.

**This plan:** the entity model keeps only **characters, and relationships between them**. The `object`, `location` and `campaign` kinds go; where a character is and what they wear become free-text character fields. Fewer kinds and fewer rules leave the narrator more room to imagine, and give it less to keep consistent.

**Old stories don't matter:** C0 deletes the database and the app creates a fresh one, so nothing in this plan reads or converts existing data.

What's missing today:
- **Character facts.** A character can't have a gender, role, location, outfit, or a true identity behind a title ("the hooded stranger" who is really Kael).
- **Relationships.** `attribute_registry` already has Trust, Fear, Affection and Respect (−10..10) for kind `relationship`, but a relationship entity has no idea **which two characters** it connects.
- **Proper tool schemas.** One generic `create_entity` / `update_entity` pair serves every kind.
- **Separate control** over recording and showing characters vs relationships.

Also, `story_entity_state` is a leftover 1:1 split of `entities`, and `last_event_id` is written but never read. C2 removes both before anything builds on them.

**Entity state is built from events.** This shapes the whole design:
- Every change is written as an `entity_*` transcript record through `entities::projection::record` (`projection.rs:128`). `projection::apply` then updates the entity tables from that record.
- Erase and Retry call `projection::replay` (`projection.rs:151`). For each affected entity, replay **deletes its `entities` row**, which cascades to its `characters`/`relationships` row and its stats, and then re-applies that entity's events from the transcript.
- So everything new must be carried in events, so that replay can rebuild it.
- And **`relationships.from_id` and `to_id` must not have a foreign key with `ON DELETE` to `entities`.** Replaying Mira deletes and re-creates her `entities` row; a cascade would silently wipe every relationship to Mira, and none of those are in the replay set.

**Schema diagram:** `docs/plans/entities-schema.drawio` shows the entity tables after this plan.

## Decisions already made (don't revisit)

### The model: a shared identity table, with one table per kind
The user chose this over separate attribute tables per kind (`character_attributes`, `relationship_attributes`) and over one attribute table with a nullable foreign key per kind. `entities` holds only what every kind has, so stats, events, replay, presence and dice keep one id space.

```sql
entities (
  id PK, story_id → stories (cascade),
  kind CHECK (kind IN ('character', 'relationship')),
  name,                              -- characters only; NULL for relationships
  is_present, created_at, updated_at,
  CHECK ((kind = 'character') = (name IS NOT NULL))
)  -- name unique per story among present entities; NULL names never clash
characters        (entity_id PK → entities (cascade), known_as, appearance_anchor, gender, age, role, location, outfit)
relationships     (entity_id PK → entities (cascade), from_id, to_id, label, direction, description,  UNIQUE (from_id, to_id))
entity_attributes (entity_id → entities (cascade), attribute_id → attribute_registry, value, source, updated_at,
                   PRIMARY KEY (entity_id, attribute_id))
```
- **Two kinds:** `character` and `relationship`. `object`, `location` and `campaign` are gone, with their starting attributes. Each kind's table is named after it (`characters`, `relationships`).
- **Every character has its `characters` row**, and every relationship its `relationships` row (1:1 with `entities`).
- **Only `entities` carries `story_id`.** `characters`, `relationships` and `entity_attributes` are keyed by `entity_id`; entity ids are unique across stories, and deleting a story cascades through `entities`. Functions that take a `story_id` keep it and scope reads by joining `entities`, so an id from another story reads as missing.
- **No `last_event_id`.** The link from the transcript is the `entity_id` inside each event's payload, which replay scans for; no column points back.
- **Entity names are unique among present entities:** `idx_entities_name_ci ON entities(story_id, name COLLATE NOCASE) WHERE is_present = 1`. A deleted name is free again. Erasing a deletion while a new entity has taken the name fails with the index error; that's accepted and tested. Names may not contain `→` or `->` (checked on create and rename), so a name is never mistaken for a relationship reference.

### Characters
- **Fields, all free text and nullable:** `known_as`, `appearance_anchor`, `gender`, `age`, `role`, `location` ("the Rusty Anchor, back room"), `outfit` ("father's grey cloak over travel leathers"). `location` and `outfit` replace the dropped placement and items with prose the narrator keeps current. The five short fields (`gender`, `age`, `role`, `location`, `outfit`) and `known_as` are at most 200 characters each.
- **Records hold the truth; narration shows what the player perceives.** The user chose this so the narrator can plan ahead: when a hooded stranger appears, it records her as **Kael** with her real look, role and relationships, sets `known_as: "the hooded stranger"`, and narrates only that until the story reveals her. A title is not a name.
  - The narrator is told so in the `save_character` description and the context header.
  - The context line shows both: `- Kael (character); known to the player as: the hooded stranger; …`.
  - **The narrator can't rename.** A reveal clears `known_as`. The player can rename in the UI (a typo, or a narrator that recorded only a title).
  - **The UI doesn't spoil reveals:** while `known_as` is set, a row's title is the `known_as` text, and the true name sits behind a collapsed "Reveal" toggle.

### Relationships
- **A relationship is an entity with two ends** (two characters), so stats (Affection, Trust) attach to its id, and dice factors, stat events and the attribute editor work on it unchanged.
- **Direction is one column:** `one_way` or `both`. Mutual facts (siblings, married) get one `both` row; feelings that differ per side get two `one_way` rows, each with its own stats.
- **At most one relationship per direction between two characters.** Several facets ("estranged sister" who also "owes you money") go in that relationship's label and description. This is what lets the narrator name a relationship as `"Mira → You"` without ids.
- **Uniqueness is enforced by the database:** `UNIQUE (from_id, to_id)` on `relationships`. It counts soft-deleted relationships too, so a relationship has a **fixed id**, `"relationship:{from_id}:{to_id}"`: re-creating it reuses the old row (`Created` upserts it back to present) with its old stats. For a `both` relationship, revival also looks for the reversed id and reuses its stored orientation.
- **The `both` rule can't be an index** ("A ↔ B excludes A → B and B → A"), so the link functions check it in code. A `both` relationship can be made one-way only in its stored orientation; a reversed request is rejected rather than silently flipping the meaning.
- **Writes use `INSERT … ON CONFLICT(entity_id) DO UPDATE`,** never `INSERT OR REPLACE`, which would silently delete a row that clashes with the unique index.
- **Display text is built on read** from the label and the endpoints' current names: `"Mira → You: estranged sister"`, or `"Mira ↔ Varro: siblings"`. Reads put it in the entity's `name` field, so the narrator, tools and UI never see a stale name.
- **Reads hide** relationships whose endpoint isn't present.
- **The narrator can't delete relationships;** one that ends gets relabelled ("former friends"). The player can delete them in the UI.

### Narrator tools
- **Two entity tools, both create-or-update, identified by name, no ids:**

  | Tool | Arguments | Step |
  |---|---|---|
  | `save_character` | `name`, `known_as?`, `appearance_anchor?`, `gender?`, `age?`, `role?`, `location?`, `outfit?`, `stats?` | C4 |
  | `save_relationship` | `from`, `to`, `label?`, `direction?`, `description?`, `stats?` | C3 |

  - A field that's left out stays as it is; `null` clears it.
  - **References** (`from`, `to`, `roll_check` factors, `illustrate_scene` characters) take a character name (an id is also accepted), resolved among present entities; a relationship is referenced as `"Mira → You"` (`->` accepted). The `<entities>` block shows names, never ids.
  - **`stats`** is a list of `{attribute, delta, reason, dramatic?}`, applied after the fields with today's attribute resolution and damping. It replaces `adjust_entity_attribute`, so stat changes follow each tool's Record switch.
- **Removed:** `create_entity`, `update_entity`, `get_entities` and `adjust_entity_attribute`. `get_entities` goes because the user treats "Show: Hidden, but the narrator can look it up" as a bug; the `entity_queried` transcript kind goes with it.
- **Kept, with name references:** `roll_check` factors take `entity` (a name or `"A → B"`) instead of `entity_id`; `illustrate_scene` takes `characters` (names) instead of `character_ids`.
- **The user's own UI commands** `create_entity` and `update_entity` stay, for characters, with the new fields.
- **One file per narrator tool,** in `narrator/tools/` (C1). Each file holds the tool's name, description, schema, handler, label, settings switch and tests.
- **Output is short:** `{"name", "kind", "created": true|false}` plus `stats` results. The narrator just wrote the fields; echoing them back only costs tokens.
- **Each save call is atomic.** Network lookups (attribute matching by embeddings) happen first with no writes; then every write of that call (entity events, registry aliases or mints, stat changes) runs in one `turn.with_savepoint`. If anything fails, none of it remains, so a retry can't apply a stat twice.

### State rules
- **Stored state vs shown state.** Presence filters (hiding relationships to absent characters) apply only to what's shown: the context, tool results, the UI. Everything that writes reads the stored rows (`load_entity_raw`) for its `before` snapshot, no-op check and validation.
- **Replay applies changes, not snapshots.** `Updated` events keep full `before`/`after` snapshots, but projecting one writes only the fields that differ. Otherwise a later, unrelated edit would carry an erased turn's facts in its snapshot and bring them back (this exists today for name and appearance; C2 fixes it).
- **Replay checks only the final state against the name index.** Replayed entities are written as not present, and one update at the end marks the finally present ones. A name used only in an entity's history can't block an erase; a genuine final clash still fails.
- **No new transcript kind.** Characters and relationships use `entity_created`, `entity_updated`, `entity_deleted` and the attribute records; "entity" means character or relationship.

### Player edits: the newer fact wins (C3)
- **The narrator's rule:** "When the story and a record disagree, the more recent one is correct. Never undo a player correction unless later story events change it." It replaces "user overrides win" (`prompts.rs:55` and `:86`).
- **Player edits are labelled and state the change.** Content of a record with `source = "user"` starts with `"User edit: "`, and update content lists the actual changes ("Mira: role pirate → innkeeper; location → the docks; gender cleared"), because history shows `content`, not the payload.
- **The permanent stat lock is removed** (`attributes.rs:77-82`, `tools.rs:378-402`): a player-set stat is an ordinary newer fact.
- Pinning fields against narrator changes is not in this plan.

### Settings: Record and Show, each in the panel that owns it (C5)

| Row | Narrator tools panel: Record | Context panel: Entities in context |
|---|---|---|
| Characters | `save_character` checkbox | Shown / Hidden |
| Relationships | `save_relationship` checkbox | Shown / Hidden |

- **Show replaces** `ContextSettings.entities` (`All` / `Scoped` / `None`); **Scoped is removed.**
- **Hidden leaves that kind out of the `<entities>` block, and that's all.** It saves context or helps experiments; it isn't a secrecy feature, so tool results aren't filtered by it.
- Everything is on and shown by default. No setting changes another.

## Ground rules

- **Start point:** a new branch `entities` from `main` at `71ef4b8`, with `git status` clean. `docs/` is git-ignored. Don't rewrite existing commits.
- **Run the whole plan in one go, without asking the user anything.** Where something is unclear, choose, and record the choice in the report.
- **Fix, don't stop.** When a check fails:
  1. Find the cause and fix it on `entities`, with a unit test when it can be reproduced in one. Commit it as "Fix …", with its line counts and cause.
  2. Rerun the checks below, then the failed check and everything after it.
  3. Up to **5 attempts per check**, within the spending cap. If a check still fails, record it with what you tried, carry on, and **don't merge into `main`**.
- **No migration code in the app.**
  - Schema changes are edits to the `CREATE` statements in `shared/db.rs`: C2 restructures `entities` and `entity_attributes` and drops `story_entity_state`; C3 adds `relationships` and the `kind`/`name` checks; C4 adds `characters` and moves `appearance_anchor` there.
  - C0 deletes the database, so the app starts from a fresh one. No `ALTER TABLE`, no copying of rows, no compatibility code or tests for old payloads or settings.
  - Before each commit, `rg -n "ALTER TABLE|pragma_table_info|table_info|migration" src-tauri/src` must have hits only in `#[cfg(test)]` code.
- **Database and secrets:**
  - The database is `%APPDATA%\com.story-llm.app\story-llm.sqlite3`. Destructive database changes are allowed, including deleting it (C0).
  - **Never** open, print, copy, overwrite or delete `secrets.json`, which sits in the same folder and holds the API keys. Any delete in that folder names the `.sqlite3*` files explicitly, never the folder or a wildcard.
  - **Never push.**
- **One commit per step** (C1–C6, plus any "Fix …"). Each message states its production and test line counts from `git diff --numstat`.
- **After every commit:**
  - `cargo test --manifest-path src-tauri/Cargo.toml`
  - `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets` with zero warnings
  - `node --test src/features/story/store.test.mjs src/features/transcript/replacement.test.mjs src/features/usage/store.test.mjs`
  - `npx tsc --noEmit`
- **The report** goes to `docs/report/entities.md` and links each evidence file by its full path. **Evidence files** go in `docs/report/entities/`. Each QA check saves what it saw (DOM captures, read-only query results, log excerpts); a check without a saved file is "no evidence", never a pass.
- **QA drives the app like a user,** with real clicks and typing through tauri-pilot. Database reads are read-only (`mode=ro`). Never edit the DOM, app state or rows to make a check pass.
- **Spending cap:** at most **15 text model calls, 0 images and 0 captions**, including fix reruns. Turn **Illustrate scenes** off in every QA story, and don't use See.
- **tauri-pilot notes:** start the app with `pnpm tauri dev`. The pipe is `\\.\pipe\tauri-pilot-com.story-llm.app`; drive it from **PowerShell** (Git Bash mangles the pipe path). PowerShell 5.1 splits double-quoted arguments with spaces: use here-strings or prefix selectors like `[aria-label^=New]`. Poll with read-only DOM captures in a loop (every 300–500 ms).

---

## C0. Start from a fresh database (no commit)

1. Close the app.
2. Read-only, save to `c0-model.txt` the current text model's provider and model name from the `settings` table, so C7 can select the same one again. Save nothing else from that table.
3. Delete `story-llm.sqlite3`, `story-llm.sqlite3-wal` and `story-llm.sqlite3-shm`, **each by name**. Don't touch `secrets.json`; confirm it still exists (existence only, never open it) and record that in `c0-model.txt`.
4. The app creates a fresh database on its next start.

---

## C1. Move the narrator tools into `narrator/tools/`, one file per tool (no behaviour change)

A pure move, so later steps add tools as new files with readable diffs. Nothing the model or the frontend sees changes.

### Layout
Start with `git mv src-tauri/src/features/narrator/tools.rs src-tauri/src/features/narrator/tools/mod.rs`, so history follows the file, then split it:
```
narrator/tools/
  mod.rs                      shared code: to_tool_error, friendly_tool_label, the submodule list,
                              and #[cfg(test)] pub(super) mod test_support (the shared fixture)
  get_entities.rs             removed in C3
  create_entity.rs            removed in C4
  update_entity.rs            removed in C4
  adjust_entity_attribute.rs  removed in C4 (its stat code moves to mod.rs in C3)
  roll_check.rs
  illustrate_scene.rs
```
**Each tool file holds everything for that tool:** its `NAME`, `DESCRIPTION` and `schema()` (moved from `prompts.rs`) and its instruction line if any (`ROLL_CHECK_AVAILABLE_INSTRUCTION`, `IMAGE_TOOL_AVAILABLE_INSTRUCTION`); the builder (`*_tool`) and label (`*_label`); its `enabled` check and `build` adapter (moved from `catalog.rs`); `pub(in crate::features::narrator) const SPEC: ToolSpec`; and its own `#[cfg(test)] mod tests`.

**`catalog.rs` keeps** `ToolAvailability`, `ToolDeps`, `ToolSpec`, `turn()`, `normal_mode()` and `enabled()`. Today `TOOLS` is `pub static TOOLS: [ToolSpec; 6]` (`catalog.rs:112`); make it `pub static TOOLS: &[ToolSpec] = &[roll_check::SPEC, get_entities::SPEC, …]`, in today's order, so adding a tool is one file and one line with no length to update.

**`dice.rs` keeps the dice mechanics** (`RollFactor`, `RollPayload`, `RollOutcome`, `chance_from_factors`, `resolve_roll`, and their tests). `roll_check_tool`, `roll_check_label` and `factor_reading` move to `tools/roll_check.rs`, with the tests "roll is written in turn…" and "factor reads current turn attribute…".

**`prompts.rs` keeps** the system prompt, turn rendering, retry, caption, title and compaction prompts, and `ENTITY_CONTEXT_HEADER`.

**Update the callers:** `catalog.rs`; `stream.rs:190` and `:203` (`friendly_tool_label`); `ai/mod.rs:296` and `:985` (`ILLUSTRATE_SCENE_TOOL_NAME`, give the moved constant `pub(crate)`); the `dice.rs` test helpers that build tools; the `blocks.rs` tests that use moved names and instructions (`blocks.rs:516-532`). The roll-schema test in `prompts.rs` (about `:274`) moves to `tools/roll_check.rs`.

### Proving nothing changed
- Before moving anything, add a throwaway test that writes every tool's name, description, schema JSON, instruction and `friendly_tool_label` for a sample argument set to `docs/report/entities/c1-tool-contract-before.json`; run it again after the move to `c1-tool-contract-after.json`. The files must be byte-identical. Don't commit the throwaway test.
- The `cargo test` count is the same before and after (record both). Tests are moved, not rewritten, apart from import paths.

---

## C2. Fold `story_entity_state` into `entities`, drop the redundant links, and make replay safe

### Schema: `shared/db.rs`
Replace the `entities` and `story_entity_state` statements and their indexes (`db.rs:110-129`) with:
```sql
CREATE TABLE IF NOT EXISTS entities (
    id TEXT PRIMARY KEY,
    story_id TEXT NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    appearance_anchor TEXT,          -- moves to characters in C4
    is_present INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_entities_story_name ON entities(story_id, name);
CREATE UNIQUE INDEX IF NOT EXISTS idx_entities_name_ci
    ON entities(story_id, name COLLATE NOCASE) WHERE is_present = 1;
```
and the `entity_attributes` statement (`db.rs:146-155`) with:
```sql
CREATE TABLE IF NOT EXISTS entity_attributes (
    entity_id TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    attribute_id TEXT NOT NULL REFERENCES attribute_registry(id) ON DELETE CASCADE,
    value REAL NOT NULL,
    source TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (entity_id, attribute_id)
);
```

### Projection: `entities/projection.rs`
- **`apply` loses its `event_id` parameter** (it only fed `last_event_id`) and gains a mode, `Live` or `Replay`. Update its callers (`record`, `replay`).
- **`Created`:** one upsert replaces the two inserts: `INSERT INTO entities (…) VALUES (…, 1, …) ON CONFLICT(id) DO UPDATE SET name, appearance_anchor, is_present = 1, updated_at`. `kind` and `created_at` keep their first values. Re-creating a soft-deleted relationship with its fixed id (C3) relies on this.
- **`Updated`:** write only `name` and `appearance_anchor` values that differ between `before` and `after`, plus `updated_at`, and in `Live` mode `is_present = 1`. C3 and C4 extend the same rule to relationship and character fields.
- **`Deleted`:** today's `UPDATE`, against `entities`, without `last_event_id`.
- **`AttributeChanged`** upserts `ON CONFLICT(entity_id, attribute_id)`; **`AttributeRemoved`** deletes by `entity_id` and `attribute_id`. Neither writes `story_id` or `last_event_id`.
- **`replay`:** drop the `DELETE FROM story_entity_state` and `DELETE FROM entity_attributes` statements (`projection.rs:149-156`); deleting the `entities` row cascades.
- **Replay presence pass:** in `Replay` mode, `Created` and `Updated` write `is_present = 0` and `Deleted` doesn't touch presence. `replay` tracks each entity's final presence from its events (created or updated → present, deleted → absent); after the loop, one `UPDATE entities SET is_present = 1 WHERE id IN (…)` marks the finally present ones.

### Reads and other writers
Each loses its join and reads `entities` directly: `entities/repository.rs:30-32` (`list_entities_sync`), `:143-146` (the "before" lookup), `:179` (`delete_entity_sync`); `entities/attributes.rs:196`; `images/generation.rs:58-62` (`characters_by_ids`); `shared/db.rs:210` (`seed_player_entity`).

**Stats reads** in `entities/attributes.rs` drop `entity_attributes.story_id` (`:29`, `:130-133`, `:160-164`, `:198`, `:223`, `:248`). Functions keep their `story_id` parameter and join `entities e ON e.id = entity_attributes.entity_id AND e.story_id = ?`. `EntityAttributeValue.story_id` stays, filled from `e.story_id`, so the frontend type is unchanged.

### Tests
- Raw SQL in tests switches to the new columns: `projection.rs:214`, `:215`, `:234`, `:235`, `attributes.rs:286`, `:348`, `stories/repository.rs:195`, `transcript/erase.rs:335`, `:342`, `:350`, `:489`, `:504`, `:515`, `:525`, `:600`, and `shared/db.rs:407`, `:415`, `:416`, `:424`, `:560`, `:568`, `:585`. Assertions on `last_event_id` are removed.
- The schema test (`db.rs:447`) asserts `story_entity_state` doesn't exist, `entities` has the new columns, and neither table has `last_event_id`; `entity_attributes` has no `story_id`.
- **New:** reading stats through `list_entity_attributes_sync` with the wrong `story_id` returns nothing.
- **New:** a soft-deleted entity's name can be reused; erasing the deletion afterwards fails with the unique-index error and leaves both rows unchanged.
- **New, final-state replay:** create A named "Mira"; the latest narration updates A's appearance; then delete A in the UI and create B named "Mira". Erasing the narration succeeds: A absent, B present.
- **New, change-only replay:** a turn renames Mira to "Mira Vale"; a later UI edit changes only her appearance. Erasing the turn restores "Mira" and keeps the new appearance.

### Proof
- `rg -n "story_entity_state|last_event_id" src-tauri/src` has hits only in `#[cfg(test)]` code.
- The `cargo test` count is C1's plus the four new tests; no other test is deleted.
- The C1 tool-contract dump is unchanged.

---

## Shared tool rules (written in C3, used by both save tools)

In `narrator/tools/mod.rs` (or `tools/shared.rs` if `mod.rs` gets long):
- **`resolve_entity(conn, story_id, reference) -> AppResult<Entity>`**, among present entities, in order: an exact id; `"A → B"` or `"A -> B"`, the relationship from A to B or a `both` one between them in either order (A and B resolved by name); a character's name, case-insensitive. Not found → `invalid_args("no entity named '{reference}'")`.
- **Field parsing:** a string sets the field, JSON `null` clears it, a missing key leaves it; any other type → `invalid_args("{field} must be a string or null")`. Strings are trimmed; an empty string counts as `null`.
- **Atomic calls, in two phases:**
  1. **Prepare, no writes:** parse and validate the arguments, and resolve every stat's attribute with `resolve_stats(turn, embedding_api_key, kind, &[StatChange]) -> Vec<ResolvedStat>` (async: exact match, or the embedding-based alias/mint decision, as `registry::resolve_attribute_in_turn` does today).
  2. **Write, in one `turn.with_savepoint`:** the entity events, then `apply_resolved_stats(conn, …)`, which re-checks the exact match, adds the alias or mints the attribute, and calls `apply_attribute_delta` per stat. Any error rolls back the whole call.
- **Stats:** today's attribute code in `adjust_entity_attribute_tool` (`tools.rs:337-402`) is split between those two functions. The player-lock check (`tools.rs:378-380`) isn't moved. `StatChange` is `{attribute: string, delta: number, reason: string, dramatic?: bool}`, at most 8 items, `additionalProperties: false`; a bad item → `invalid_args` naming its index. Each result is `{attribute, before, after}`. The `stats` schema fragment is one function used by both tools.
- **Output:** `{"name", "kind", "created": true|false}`, plus `"stats"` results when given.
- **Source and turn:** entity writes use `"narrator_tool"` and the turn's target entry and turn id, as `create_entity_tool` does today; stat changes keep the source `"inferred"`.
- **Schemas** set `additionalProperties: false`.
- **Builders take `embedding_api_key`** (for `stats`); their build adapters pass `deps.embedding_api_key`, as `build_adjust_entity_attribute` does today (`catalog.rs:103`).

---

## C3. Relationships, `save_relationship`, and the narrowing to characters

### Schema: `shared/db.rs`
- **`entities` gets its checks:** `kind TEXT NOT NULL CHECK (kind IN ('character', 'relationship'))`, `name` becomes nullable, and the table gains `CHECK ((kind = 'character') = (name IS NOT NULL))`.
- **Add after `entities`:**
  ```sql
  -- from_id/to_id have no foreign key on purpose: replay deletes and re-creates
  -- entity rows, and a cascade would wipe relationships that replay doesn't rebuild.
  CREATE TABLE IF NOT EXISTS relationships (
      entity_id TEXT PRIMARY KEY REFERENCES entities(id) ON DELETE CASCADE,
      from_id TEXT NOT NULL,
      to_id TEXT NOT NULL,
      label TEXT NOT NULL,
      direction TEXT NOT NULL CHECK (direction IN ('one_way', 'both')),
      description TEXT
  );
  CREATE INDEX IF NOT EXISTS idx_relationships_to ON relationships(to_id);
  -- One relationship per direction. Counts soft-deleted rows, so re-creating
  -- must reuse the fixed id (relationship_id in model.rs).
  CREATE UNIQUE INDEX IF NOT EXISTS idx_relationships_pair ON relationships(from_id, to_id);
  ```
- **Attribute starters:** keep the `character` and `relationship` lines; remove every `object`, `location` and `campaign` line (`db.rs:246-255` and `:260`).
- **Kind names elsewhere, same commit:** `EntityKind` in `src/shared/types.ts` becomes `"character" | "relationship"`; the kind lists in `CREATE_ENTITY_*` (its C1 file) become `character` only; the `"location"` test fixture in `projection.rs` (about `:378`) becomes a character. Afterwards `rg -n '"(object|location|campaign)"' src-tauri/src src` finds only JSON-schema `"type": "object"` and `typeof … "object"` checks.

### Model: `entities/model.rs`
```rust
pub const CHARACTER: &str = "character";
pub const RELATIONSHIP: &str = "relationship";
pub fn relationship_id(from_id: &str, to_id: &str) -> String { format!("relationship:{from_id}:{to_id}") }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityLink { pub from_id: String, pub to_id: String, pub label: String,
                        pub direction: String /* "one_way" | "both" */, pub description: Option<String> }
```
- `Entity` gains `#[serde(default)] pub link: Option<EntityLink>`. `Entity.name` stays a `String`: for relationships, reads fill it with the display text (below).

### Events: `events.rs`
- `Created` gains `link: Option<EntityLink>`, and its `name` becomes `Option<String>` (None for relationships). `payload()` adds `"link"` only when `Some`; `from_entry` reads a missing key as `None`.
- `NameAnchor` gains `link: Option<EntityLink>`, so `Updated.before`/`.after` carry it.

### Projection
- **`Created` with a link:** `INSERT INTO relationships (…) ON CONFLICT(entity_id) DO UPDATE SET from_id, to_id, label, direction, description`, after the `entities` upsert.
- **`Updated`:** write only the link fields (`label`, `direction`, `description`) that differ between `before.link` and `after.link`; endpoints never change.
- **Replay needs nothing new:** replaying a relationship deletes its `entities` row, which cascades its `relationships` row; replaying an endpoint doesn't touch `relationships`.

### Repository: `repository.rs`
- **One shared `SELECT` for shown state**, used by `list_entities_sync` and everything that shows entities:
  ```sql
  SELECT e.id, e.story_id, e.kind,
         COALESCE(e.name, f.name || CASE l.direction WHEN 'both' THEN ' ↔ ' ELSE ' → ' END || t.name || ': ' || l.label) AS name,
         e.appearance_anchor, e.created_at,
         l.from_id, l.to_id, l.label, l.direction, l.description
  FROM entities e
  LEFT JOIN relationships l ON l.entity_id = e.id
  LEFT JOIN entities f ON f.id = l.from_id AND f.is_present = 1
  LEFT JOIN entities t ON t.id = l.to_id AND t.is_present = 1
  WHERE e.story_id = ?1 AND e.is_present = 1
    AND (l.entity_id IS NULL OR (f.id IS NOT NULL AND t.id IS NOT NULL))
  ```
  `row_to_entity` maps the link columns to `link` when `from_id` isn't NULL.
- **`load_entity_raw(conn, story_id, entity_id) -> AppResult<Option<Entity>>` for stored state:** the same columns and `row_to_entity` for one present entity, without the endpoint presence filter (if an endpoint is gone, the display name falls back to the relationship's id). Every write path uses it for `before`, no-op checks and validation: `update_entity_sync`, `update_link_sync`, and C4's `update_character_sync`.
- **Kind and name checks:** `create_entity_with_id_sync` (keep its signature; it has many test callers) accepts only `character` (`AppError::Invalid("kind must be character")`), so the user's `create_entity` command can't make relationships. Creating or renaming rejects a name containing `→` or `->` (`"names can't contain arrows"`).
- **New `create_link_sync(conn, story_id, link: EntityLink, source, target_entry_id, turn_id) -> AppResult<Entity>`.** It checks: `label` non-empty after trimming; `direction` is `one_way` or `both`; `from_id != to_id`; both endpoints are present characters in this story; the `both` rule (creating `both` when a present `one_way` exists for the pair in either order, or `one_way` when a present `both` exists → `AppError::Invalid` naming the existing relationship).
  - **Id and revival:** the id is `relationship_id(from_id, to_id)`; when creating `both`, also look for a soft-deleted `relationship_id(to_id, from_id)`. If a soft-deleted row exists, reuse its id and stored orientation; `Created` revives it with its old stats.
  - Records `Created { kind: "relationship", name: None, link: Some(..) }` with content `"{from} → {to}: {label} (relationship recorded)."`. Share the event-building code with `create_entity_with_id_sync` through one private helper.
- **New `update_link_sync(conn, story_id, entity_id, label, direction, description, source, target_entry_id, turn_id) -> AppResult<Entity>`.** It validates like `create_link_sync` (including the `both` rule when `direction` changes), keeps the endpoints, and records `Updated` with `before.link`/`after.link` and content listing the change (`"Mira → You: label → former friends."`). A `both` relationship can be made `one_way` only in its stored orientation; a reversed request → `AppError::Invalid("this relationship is stored as {a} ↔ {b}; make it one-way in that direction, or record the other direction separately")`.
- **`delete_entity_sync`** works on relationships unchanged.

### Tool settings: `stories/settings.rs`
- `NarratorToolSettings` gains `save_relationship: bool` (default `true`) and loses `get_entities`. Add struct-level `#[serde(default)]` (using the `Default` impl); keep `deny_unknown_fields`.
- `types.ts` `NarratorToolSettings` gains `save_relationship` and loses `get_entities`; `NarratorToolsPanel.tsx` gains "Record relationships" and loses "Look up entities"; `store.test.mjs` drops `get_entities` from its fixture (`:55`) and uses another key in the save test (`:320`, `:324`).

### Narrator tool: `save_relationship` (new file `narrator/tools/save_relationship.rs`, one line in `TOOLS`)
- **Schema:** `from`, `to` (string, required: a character name); `label` (string, required when creating); `direction` (enum `one_way`/`both`, default `one_way` when creating); `description` (string or null); `stats` (the shared fragment); `additionalProperties: false`.
- **Description:**
  > Record how one character relates to another, or update it. There is one relationship per direction: calling again with the same from and to updates it (a new label replaces the old one, such as "former friends" when a friendship ends). `label` is short ("estranged sister", "owes money to", "siblings"). Use direction "both" only for truly mutual facts (siblings, married, allies); feelings that differ per side are two one_way relationships. Use stats for how one feels about the other, such as Affection or Trust.
- **Behaviour (two phases, as in the shared rules):** resolve `from` and `to` by name; find the present relationship from→to, or a `both` one between them in either order.
  - **Found:** apply `label`, `direction` and `description` where given and different, through `update_link_sync`; `created: false`.
  - **Not found:** `label` is required (`invalid_args("label is required for a new relationship")`); `create_link_sync`; `created: true`.
  - Then `apply_resolved_stats` on the relationship, in the same savepoint.
  - The output's `name` is the display text.
- **Catalog:** enabled by `normal_mode && settings.save_relationship`; label `"Recording a relationship…"`; instruction: "Use save_relationship when the story establishes or changes how two characters relate."

### Other tool and prompt changes in C3
- **Remove `get_entities`** completely:
  - **production:** `tools/get_entities.rs` (its builder, label, name, description, schema, `enabled` check and `build` adapter, all moved there in C1), its line in `TOOLS`, `NarratorToolSettings.get_entities` (`settings.rs:15`, `:26`), and the frontend row and type (above)
  - **its own tests** are deleted with the file
  - **tests that read through it switch to the repository:** `entity_tools_write_and_read_on_the_turn_connection` (`tools.rs:494`, the call at `:527`) and `attribute_tool_applies_delta_and_honors_player_lock` (`tools.rs:554`, the snapshot at `:579`) read with `list_entities_sync` and `list_entity_attributes_sync` on the turn connection instead
  - **test fixtures that name it:** the `NarratorToolSettings` literals in `blocks.rs:493` and `:557` drop the field; the example tool name in `blocks.rs:455`, `preview.rs:107` and `ai/mod.rs:989` becomes `roll_check` (with its instruction text where one is given)
  - the old `tool_call` payloads in `history.rs:600` and `store.test.mjs:870` are recorded data, not tool definitions; leave them
  - **texts in the tools that stay until C4 that mention it:** `update_entity`'s description ("look it up first when the lookup tool is available", `prompts.rs:207`) and its `id` description ("from get_entities/create_entity", `:213`) drop the lookup wording; `adjust_entity_attribute`'s entity description (`:231`) changes with its switch to names (below)
  - afterwards `rg -n "get_entities|GET_ENTITIES" src-tauri/src src` finds only those two recorded payloads
- **Remove the `entity_queried` transcript kind:** `kind::ENTITY_QUERIED` and its place in `RECORD_KINDS`, the "Entity lookups" item in `filter.rs`, the `query.rs:76` kind list, and `"entity_queried"` in `types.ts` and `TurnActivity.tsx`. Tests using it as a sample record kind (`blocks.rs:231`, `preview.rs:189`, `erase.rs:417`, `history.rs:434`, `:591`, `:665`) switch to `ENTITY_UPDATED`.
- **`roll_check`:** each factor's `entity_id` becomes `entity` ("A character name, or a relationship as 'Mira → You'"), resolved with `resolve_entity`; the stored roll payload keeps the resolved id. Drop "preferably from get_entities" and "do not invent entity IDs" from its texts, and "Use get_entities to find IDs…" from `ROLL_CHECK_AVAILABLE_INSTRUCTION`.
- **`illustrate_scene` takes names:** schema property `character_ids` becomes `characters` ("Names of characters visible in the scene."), replacing the "from get_entities" text (`prompts.rs:133`, now in its C1 file); its parsing (`tools.rs:79-106`) reads `characters`. `ImageRequest.character_ids` keeps its internal name. `characters_by_ids` (`images/generation.rs:45-75`) matches a present character by id **or** by name, case-insensitively.
- **The stat code moves to `tools/mod.rs`** as `resolve_stats` / `apply_resolved_stats`. `adjust_entity_attribute` (removed in C4) uses them for its single change and takes `entity` by name, so it keeps working in between.
- **`create_entity` tool (until C4):** its `kind` becomes the enum `["character"]`.
- **`ENTITY_CONTEXT_HEADER` adds:** "A → B relationships are one-way; A ↔ B are mutual. Refer to characters by name, and to a relationship as 'A → B'."
- **Remove the leftovers of the old classify/resolve/update dice pipeline** (replaced by the tool-calling narrator in `64bc008`, 2026-09-16). Nothing reads any of them:
  - **the `campaign` kind:** it was a story-wide difficulty dial for that pipeline's roll modifier. Remove it from the tool descriptions (`prompts.rs:182`, which goes with `get_entities`, and `:197`, which becomes `["character"]` above), from `EntityKind` (`types.ts:117`), and its seeded `("Difficulty", &["campaign"], …)` attribute (`db.rs:260`). These are the same edits as the starter and kind-name bullets in this step; list them in the report as one item.
  - **the `attributes_enabled` and `dice_mode` story settings:** `create_story_in_pool` still strips them (`stories/repository.rs:77-78`). Delete those two lines, and drop the two keys and their two assertions from the test at `stories/repository.rs:181-182` and `:190-191`.
  - Afterwards `rg -n -i "campaign|dice_mode|attributes_enabled" src-tauri/src src` finds nothing.

### Player edits: newer wins
- **Prompts:** replace "`<entities>` is authoritative, and user overrides win." (`prompts.rs:55`) with "`<entities>` is the current record of the story's characters and relationships. When it disagrees with the story, the more recent one is correct; never undo a player correction unless later story events change it." Replace `ENTITY_CONTEXT_HEADER` (`prompts.rs:86`) with "Current record. When the story and a record disagree, the more recent one is correct. Never undo a player correction unless later story events change it."; the relationship sentence above and C4's true-name sentence follow it.
- **Labelled content:** add `fn recorded_content(source: &str, text: String) -> String` in `entities/repository.rs` (`"User edit: {text}"` when `source == "user"`). Every entity event content goes through it: the shared create helper, `update_entity_sync`, `delete_entity_sync`, `update_link_sync`, and C4's `update_character_sync`. Stat content already says "User changed …".
- **Content states the change:** update content is built from the `before`/`after` diff (`"Mira: name Old Tom → Tom; appearance → \"grey eyes, scarred lip\"."`), replacing today's "appearance details were updated".
- **Stat lock removed:** delete the `current_source == "user"` early return in `apply_attribute_delta` (`attributes.rs:77-82`, with its comment) and the matching check in the moved tool code.
- **Unchanged stat sets record nothing:** `set_entity_attribute_sync` (`attributes.rs:195-215`) returns the current value without an event when the new value equals the stored one, like the no-op rule for character and relationship updates. Otherwise a repeated save writes "User changed Health from 7 to 7" into the narrator's history.

### Narrator context: `context/blocks.rs` `format_entity_context`
- A relationship's line is `- Mira → You: estranged sister (relationship); description: …; attributes: Affection=8, Trust=-2`, with `↔` for `both`.
- In scoped mode (removed in C5), a relationship that wasn't touched becomes `- Mira → You: estranged sister`.

### Tests (C3)
1. **`projection.rs`, replaying an endpoint keeps its relationships:** Mira and Varro, Mira→Varro "rivals", Affection −3 on it; snapshot `relationships` and its stats; replay `{mira}`, then the relationship; snapshots identical each time. (The foreign-key trap; this test must exist.)
2. **`projection.rs`:** erasing the turn that created a relationship removes it and its stats.
3. **`repository.rs`, `create_link_sync` rejects** a missing endpoint, `from == to`, a relationship endpoint, a bad direction, an empty label, and `both` while a reverse `one_way` exists; creating a non-character through `create_entity_with_id_sync` is rejected; a name with an arrow is rejected.
4. **Uniqueness in the database:** a second `relationships` row for the same `(from, to)` with a different id fails (raw SQL); deleting Mira → You and recording it again revives the same id with its old stats; a deleted Mira ↔ Varro recreated as `from: Varro, to: Mira, direction: both` revives the same id.
5. **`repository.rs`:** a relationship to a soft-deleted endpoint disappears from `list_entities_sync` and comes back if the deletion is erased; its display name uses the endpoints' current names after a rename.
6. **`tools/save_relationship.rs`:**
   - endpoints by name; calling twice with the same `from`/`to` returns `created: false` (also reversed, for `both`)
   - a new label relabels it; a new relationship without `label` is rejected
   - Mira ↔ Varro updated with `from: Varro, to: Mira, direction: one_way` is rejected with the orientation message
   - `stats: [{attribute: "Affection", delta: 2, reason: "…"}]` sets Affection and returns it
   - **atomic:** a call with a new description and two stats, where the second stat's resolution fails (empty embedding key, candidates present), leaves no description change and no first stat
   - an unknown endpoint returns "no entity named"
7. **`tools/roll_check.rs`, `tools/mod.rs`:** a factor by name and by `"Mira → You"` reads the right entity; `->` works. **`catalog.rs`:** no `get_entities` tool; no `record.entity_queried` transcript item.
8. **`settings.rs`:** a tool-settings object without `save_relationship` reads it as `true`; an unknown key is still rejected.
9. **`blocks.rs`:** a relationship renders with `→`, `↔` and its attributes; the entities block starts with the new header.
10. **`images/generation.rs`:** an image prompt with `characters: ["mira"]` includes Mira's appearance block; an unknown name adds none. No image call.
11. **Player edits:** setting a stat to its current value records no event (`attributes.rs`). `tools.rs:554` `attribute_tool_applies_delta_and_honors_player_lock` becomes "applies delta over a player-set value" (player sets 7, narrator −2 → 5, source `inferred`). `history.rs`: a UI rename after a narration turn appears after it with content starting `"User edit: "` and stating the new name; the same rename by a narrator tool has no prefix.
12. **Existing test change:** `minted_attribute_is_immediately_available_to_later_tools` uses kind `"artifact"`; switch it to a character, and because `character` has starting attributes, first delete the registry rows whose kinds include `character` (test-only SQL) so minting stays deterministic without an embedding call.

---

## C4. The `characters` table and `save_character`

### Schema: `shared/db.rs`
- Remove `appearance_anchor` from the `entities` statement and add:
  ```sql
  -- Character facts. Every character has exactly one row.
  CREATE TABLE IF NOT EXISTS characters (
      entity_id TEXT PRIMARY KEY REFERENCES entities(id) ON DELETE CASCADE,
      known_as TEXT,             -- how the player knows them before the name is revealed
      appearance_anchor TEXT,    -- stable look, used for images
      gender TEXT,
      age TEXT,
      role TEXT,                 -- "innkeeper", "smuggler"
      location TEXT,             -- "the Rusty Anchor, back room"
      outfit TEXT                -- "father's grey cloak over travel leathers"
  );
  ```

### Model: `model.rs`
```rust
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CharacterFields {
    pub known_as: Option<String>, pub appearance_anchor: Option<String>,
    pub gender: Option<String>, pub age: Option<String>, pub role: Option<String>,
    pub location: Option<String>, pub outfit: Option<String>,
}
// CharacterPatch: the same fields as Option<Option<String>> (None = unchanged, Some(None) = clear).
```
- `Entity` replaces `appearance_anchor` with `#[serde(flatten)] pub character: CharacterFields`, so the JSON keeps `appearance_anchor` at the top level and gains the other fields (all null for relationships).
- Field names and columns come only from these definitions, never from user or model input.

### Events: `events.rs`
- `Created` replaces `appearance_anchor` with `character: CharacterFields`; `NameAnchor` does the same, so `before`/`after` carry every field. `payload()` writes only set fields; `from_entry` reads missing ones as `None`.
- **Projection:** `Created` for a character inserts its `characters` row (`ON CONFLICT(entity_id) DO UPDATE`). `Updated` writes only the character fields that differ between `before` and `after`.

### Reads and writers
- The shared `SELECT` and `load_entity_raw` join `characters c ON c.entity_id = e.id` and map its columns into `character`.
- `images/generation.rs` `characters_by_ids` reads `appearance_anchor` from `characters`; `context/blocks.rs` and the UI read it from `character`.
- **Create:** the shared create helper takes `CharacterFields`; `create_entity_with_id_sync` keeps its signature and passes the anchor only; new `create_character_sync(…, fields)` is used by the tool and the command.
- **Update:** new `update_character_sync(conn, story_id, id, name: Option<&str>, patch: &CharacterPatch, source, target_entry_id, turn_id)`. `before` comes from `load_entity_raw`; no event when nothing changes; content lists the changes through `recorded_content` ("Mira: location → the docks; outfit cleared"). `update_entity_sync` keeps its signature and calls it.
- **Validation:** `known_as` and the five short fields are at most 200 characters; trimmed; empty counts as `null`.

### Commands: `entities/commands.rs`
- `create_entity` gains `fields: Option<CharacterFields>` (characters only); `update_entity` gains `fields: Option<CharacterPatch>`. Renaming stays here, for the player.

### Narrator tool: `save_character` (new file `narrator/tools/save_character.rs`, one line in `TOOLS`)
- **Builder:** `save_character_tool(turn, target, turn_id, embedding_api_key)`.
- **Schema:**
  - `name` (string, required): "The character's true name, even if the player doesn't know it yet. An existing name updates that character."
  - `known_as` (string or null): "How the player currently knows them when they don't know the name, e.g. 'the hooded stranger'. Set null when the name is revealed."
  - `appearance_anchor` (string or null): "Stable look for images: face, hair, build, signature features."
  - `gender`, `age`, `role` (string or null), with examples
  - `location` (string or null): "Where they are now, in a few words, e.g. 'the Rusty Anchor, back room'. Update it when they move."
  - `outfit` (string or null): "What they're wearing now. Update it when it changes."
  - `stats` (the shared fragment); `additionalProperties: false`
- **Description:**
  > Record a character the story has established, or update one. Always use the character's true name, decided when you introduce them, even if the player doesn't know it yet; put how the player knows them in known_as and narrate them only that way until revealed. Keep location and outfit current as the story moves. Fields you leave out stay as they are; null clears a field. Use stats for changes such as an injury; most changes are minor, and dramatic is only for a major, story-changing swing. Only set facts the story has established or that you have decided for a character you're introducing.
- **Behaviour (two phases):** look up a present character named `name` (case-insensitive):
  - **found:** `update_character_sync` with the given fields (no event if nothing changed); `created: false`
  - **none:** `create_character_sync`; `created: true` (relationships have no stored name, so they can't match)
  - then `apply_resolved_stats` on the character, in the same savepoint
- **Catalog:** enabled by `normal_mode && settings.save_character`; label `"Recording {name}…"`; instruction: "Use save_character for new or changed characters, including where they are and what they wear."
- **Removed:** `tools/create_entity.rs`, `tools/update_entity.rs`, `tools/adjust_entity_attribute.rs` and their lines in `TOOLS`. The `roll_check` test helper that created a character (formerly `dice.rs:423`) uses `save_character_tool`. The attribute tests from `adjust_entity_attribute.rs` (minting, aliases, the change over a player-set value) move to `save_character.rs` and call it with `stats`.

### Tool settings
- `NarratorToolSettings`: `create_entity`, `update_entity` and `adjust_entity_attribute` are replaced by `save_character` (default `true`).
- Same commit: `types.ts` `NarratorToolSettings`; the Entity group in `NarratorToolsPanel.tsx` ("Record characters" and "Record relationships" only; the intro "All six tools start on…" becomes "All tools start on for every story…"); `store.test.mjs` keys. `types.ts` `Entity` gains the character fields (flattened); `CharactersPanel.tsx` keeps reading `appearance_anchor` unchanged.

### Narrator context
- A character's line, fields in this order, each only when set: `- Kael (character); known to the player as: the hooded stranger; location: the Rusty Anchor; outfit: grey cloak; gender: female; age: 34; role: smuggler; appearance: …; attributes: Health=7`.
- `ENTITY_CONTEXT_HEADER` gains: "Names here are true names. When an entry says 'known to the player as', narrate it only that way until the story reveals the name."
- Scoped summary lines show only the name and `known_as`.

### Tests (C4)
1. **`projection.rs`:** replaying a character created with some fields and updated with others rebuilds its `characters` row exactly; erasing the update restores the earlier values. A narration changes Mira's role; a later UI edit changes only her age; erasing the narration restores the old role and keeps the new age.
2. **`repository.rs`:** a patch that sets one field, clears another and leaves a third keeps the third; an update that changes nothing records no event; a field over 200 characters is rejected.
3. **`tools/save_character.rs`:**
   - a new name creates the character with its fields, `created: true`; the same name with a new field updates it, `created: false`, with one `entity_updated` whose content states the change
   - `{"role": null}` clears the role and leaves the rest
   - `known_as: "the hooded stranger"` is stored; a later `known_as: null` clears it ("known as cleared")
   - the schema has no `new_name`; an unknown argument is rejected
   - stats on a new character apply after creation; a change over a player-set value applies; an unknown attribute is minted; a bad stats item names its index
4. **`catalog.rs`:** with `save_character` off, the enabled tools don't contain it; no `create_entity`, `update_entity` or `adjust_entity_attribute` tool exists.
5. **`blocks.rs`:** the character line shows its fields in order, including "known to the player as"; the header has the true-name rule.
6. **`history.rs`:** with Characters hidden in the entities block and entity records on in Transcript, a UI change of Mira's role puts the new role in the narrator history.
7. **`images/generation.rs`:** the appearance block reads `characters.appearance_anchor`.

---

## C5. Show settings in the Context panel, and removing Scoped

### Backend: `stories/settings.rs`
- **Remove `EntityContext`** (`All` / `Scoped` / `None`) and `ContextSettings.entities`.
- **Add:**
  ```rust
  #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
  #[serde(default)]
  pub struct EntityVisibility { pub character: bool, pub relationship: bool } // Default: both true
  ```
  and `ContextSettings.entity_kinds: EntityVisibility`. `write_context_settings` and its command take the new field.

### Backend: `context/blocks.rs`
- **Remove scoped mode:** the `EntityContext::Scoped` branch (`blocks.rs:154-165`), `touched_entity_ids` (`blocks.rs:72`), the `Option<&touched>` parameter of `format_entity_context` and its summary-line code, and `transcript::query::entities_touched_since` (`query.rs:61`) with its test. `raw_tail_boundary` stays (compaction and `stream.rs:83` use it). `ContextPlan.live` uses the same entity block as `full`.
- **Filtering** in `format_entity_context`: hidden characters get no lines; relationships are left out when `relationship` is hidden (a shown relationship still names hidden characters); when every line is filtered out there's no `<entities>` block.
- Retire the old `None` check (`blocks.rs:121` and `:136`): loading entities is skipped only when both are hidden.

### Frontend
- **`types.ts`:** `ContextSettings` drops `entities` and gains `entity_kinds: { character: boolean; relationship: boolean }`.
- **`ContextPanel.tsx`:** replace the "Entities" select (`ContextPanel.tsx:26-33`) with a fieldset **"Entities in context"**: rows Characters and Relationships, each a radio pair "Shown" / "Hidden" (`name` unique per row, `aria-label` like "Characters shown"), saved through the existing `toggle(...)`. A muted line under it: "Hidden kinds are left out of the entities block. The narrator still sees their changes in history if the Transcript panel includes entity records."
- **`store.test.mjs`:** update the context-settings fixtures.

### Tests (C5)
1. **`settings.rs`:** the scoped tests (`settings.rs:252`, `:519`) are removed or converted; a partial `entity_kinds` (`{"relationship": false}`) reads `character` as `true`.
2. **`blocks.rs`:** with relationships hidden, no `→` line appears and characters still do; with characters hidden, no character line appears; with both hidden, no `<entities>` block; scoped tests removed or converted.
3. **`preview.rs`:** its fixture (`preview.rs:118`) drops `"entities": "scoped"`.

---

## C6. Characters panel: characters and relationships in the UI

### Types and API
- **`src/shared/types.ts`:** `Entity` gains `link: EntityLink | null` (character fields came in C4); add `interface EntityLink { from_id; to_id; label; direction: "one_way" | "both"; description: string | null }` and the labels Known as, Gender, Age, Role, Location, Outfit.
- **New command `list_story_attributes(story_id)`** (`entities/commands.rs`): every stat of the story's present entities, through `list_entity_attributes_for_entities_sync`.
- **`src/features/characters/api.ts`:** `list` calls `list_entities` with `kind: null`; `create` and `update` take character fields; `listStoryAttributes` calls the new command. Keep the file path.

### Store: `src/features/story/store.ts`
- Rename `characters`/`charactersLoading` to `entities`/`entitiesLoading`, and the actions to `loadEntities`, `createEntity`, `updateEntity`, `deleteEntity`. Add `attributesByEntity`, loaded with the entities.
- **Reload entities and attributes** after a turn completes (`store.ts:397`, `:519`), after any create, update or delete, and after a stat edit. **Deleting reloads instead of filtering the cached array** (today's `store.ts:632`), because deleting Mira also hides her relationships.
- `store.test.mjs`: the renames, plus tests that deleting a character reloads the list (a relationship to it disappears) and that attributes reload after a stat edit.
- **Queue tests** (in `store.test.mjs`, or a small test next to the queue module), with mocked commands whose delays are controlled by the test:
  - a slow set followed by a fast remove: the remove runs after the set, and the stat ends up removed
  - Save clicked while a set is pending: that set is sent once
  - Delete clicked while a set is pending: the set finishes before the delete starts
  - a failed operation doesn't block the ones queued after it

### Panel: `src/features/characters/CharactersPanel.tsx`
- **Tabs:** **Characters · Relationships**, buttons with `aria-pressed`, filtering by kind. "+ New character" only on Characters.
- **Character row:** the title is `known_as` while set, with the true name behind a collapsed "Reveal" toggle (a button with `aria-expanded`), otherwise the name; then muted lines for location and outfit, a details line (`female · 34 · smuggler`), and the appearance line.
- **Relationship row:** `Mira → You` or `Mira ↔ Varro`, the bold label, the muted description, and its stats inline (`Affection 8 · Trust -2`) from `attributesByEntity`.
- **Create form and character edit card:** inputs for name, Known as, appearance, Gender, Age, Role, Location, Outfit. Saving sends only changed fields; an emptied field is sent as `null`. The name stays editable (renaming is the player's).
- **Relationship edit card:** the label input saves through `update_entity`; the command sees `kind = relationship` and calls `update_link_sync` (add this branch, about +5 lines). The description and direction are shown read-only.
- **Both kinds:** "Add attribute…" filters the registry by the entity's own kind instead of the hard-coded `"character"`. Delete works for both.
- **Stat changes run in order, through one queue per entity.** Today a stat input saves on blur (`CharactersPanel.tsx:195`) and × removes at once (`:197`); clicking × right after typing starts both, and the two async commands can finish in either order, so the save can recreate the removed stat. Clicking Save right after typing also re-sends the pending change (`submitEdit`, `:73-79`).
  - Add `enqueue(entityId, operation)`: a promise chain per entity id (in the panel or the store), so each operation starts only after the previous one for that entity has settled, whether it succeeded or failed.
  - The blur save, ×, "Add attribute…", Save and Delete all go through it, for character and relationship edit cards alike.
  - Save and Delete run as queued operations themselves, so they start after pending stat changes; Save then compares drafts against the values those changes returned, so a pending change is never sent twice.
  - Behaviour is otherwise unchanged: stats still save on blur, and × still removes at once.
- **Out of scope:** creating relationships, and editing a relationship's description or direction, from the UI.

---

## C7. QA in the running app (tauri-pilot)

**Setup:**
1. C0 done, app closed.
2. Start the app (fresh database). In Settings, select the model from `c0-model.txt`.
3. Create a new story. In Narrator tools, turn **Illustrate scenes off**; leave Record characters and Record relationships on.
4. Save `c7-00-setup.json`: story id, tool and context settings, and a read-only `sqlite_master` capture showing `characters` and `relationships` exist and `story_entity_state` doesn't.

| # | Check | Pass when (saved evidence) |
|---|---|---|
| 1 | Fresh database and defaults | `sqlite_master` has `characters` and `relationships` and no `story_entity_state`; `attribute_registry` has no `object`, `location` or `campaign` rows. Narrator tools shows two Record boxes ticked; Context shows two Entities in context rows on Shown; saved settings have no `create_entity`, `update_entity`, `get_entities`, `adjust_entity_attribute` or `entities` key; Transcript has no "Entity lookups" item. Characters lists only the seeded "You"; Relationships is empty. |
| 2 | UI creates a character with fields | "+ New character" "Old Tom" with Role "innkeeper", Location "behind the bar" and Outfit "stained apron", by real clicks and typing. Pass when a `characters` row has those values, the `entity_created` payload carries them, and the row shows them. |
| 2b | UI edits and clears fields | Edit Old Tom: set Gender "female", save; then empty Gender and save. Pass when `gender` is NULL, `role` unchanged, and the two `entity_updated` records state the changes ("gender → female", "gender cleared"). |
| 2c | Player edits are labelled | Rename "Old Tom" to "Tom" in the UI. Pass when the record's content starts with `"User edit: "` and names "Tom", and the preview's history shows it as an authoritative event. No text calls. |
| 3 | The narrator uses the tools | **Story:** "My estranged sister Mira waits for me in the Rusty Anchor tavern, wearing our father's grey cloak. She still resents me for leaving." Pass when, with their `tool_call` records: character Mira (`save_character`; "You" already exists and may be updated), a relationship Mira→You (`save_relationship`), and Mira's `location`/`outfit` set to something matching the tavern and the cloak (recorded as seen). No `tool_call` names a removed tool. If some are missing, send one **Guide**: "Record Mira, where she is, what she wears, and her relationship to me, using your tools." At most 2 attempts; anything still missing is "narrator didn't call", not a code failure. |
| 3b | True name behind a title | **Story:** "A hooded stranger in the corner hasn't taken her eyes off us." Pass when a new character's `name` isn't a title and its `known_as` describes the stranger, the narration doesn't use the name, and the row shows the `known_as` title with the name behind "Reveal". If only a title is recorded, it's "narrator didn't call", with its tool call. One text call. |
| 4 | Stats on a relationship | **Do:** "I apologize to Mira and mean it." Pass when `entity_attributes` has a row for the relationship's id (Affection or Trust) with its `entity_attribute_changed` record, written through `save_relationship`'s `stats`, and the Relationships tab shows it inline. |
| 5 | The narrator sees the record | "Preview next request", plus read-only `preview_story_context`. The entities block contains `Mira → You:` with its attributes, Mira's `location`/`outfit`, and the stranger's "known to the player as". |
| 5b | Record and Show switches | By real clicks: untick **Record relationships**; in Context set Relationships to **Hidden**. Pass when the saved `narrator_tools.save_relationship` is `false`, `context.entity_kinds.relationship` is `false`, and the preview has no `save_relationship` tool and no `→` line while Mira's line remains. Set both back and confirm the preview matches check 5. No text calls. |
| 6 | Moving | **Do:** "Mira and I step outside onto the docks." Pass when an `entity_updated` record for Mira changes `location` to something with the docks, its content states it, and her row shows it. |
| 7 | Retry the move turn | Retry check 6's turn. Only the retried turn's records remain; Mira's location matches the retried narration. |
| 8 | Erase the move turn | Erase check 7's turn. Mira's location is back to the tavern in the database and the UI. |
| 9 | Deleting a character hides its relationships | Delete Mira in the UI. The Relationships tab (reloaded, not cached) and the preview no longer show Mira→You; the `relationships` row is still there (soft delete). |
| 10 | Persistence | Restart the app and reopen the story. Tabs, character fields, `known_as`, relationship stats, and the Record and Show settings are unchanged. |
| 11 | Logs | No backend `ERROR` or panic and no frontend error, other than ones the checks cause on purpose. Save an excerpt. |

---

## C8. Merge

If every check from C1 to C7 passes, except "narrator didn't call" outcomes explained in the report: `git checkout main && git merge --ff-only entities`; **don't push**. Otherwise leave `main` alone and say why in the report.

---

## Report: `docs/report/entities.md`

- **Result table:** one row per check (C0, the C1 contract comparison and test counts, the four checks after each commit, C7 1–11 including 2b, 2c, 3b, 5b), each PASS / FAIL / NO EVIDENCE / NARRATOR DIDN'T CALL with the full path of its evidence file.
- **Line counts:** actual per commit against the audit below, with the reason for any difference over 25%.
- **The narrator's tool calls from checks 3 and 3b,** verbatim, and whether it used true names, `known_as`, sensible labels and directions.
- **Every choice made** where the plan was unclear.
- **Spending:** text calls used, and total cost from `usage_records`.

---

## Line count audit

Estimates, based on the current code. Tests counted separately. "Net" subtracts removed lines.

| Step | Production lines | Test lines | Where the lines come from |
|---|---|---|---|
| C0 | 0 | 0 | delete the database |
| **C1 (tool split)** | **+40 to +70** net | **−5 to +10** | moved, not written: `use` lines in 6 tool files, the `mod` list, `SPEC` consts in place of `catalog.rs` entries |
| **C2 (fold, drop redundant links, safe replay)** | **−20 to 0** net | **+30 to +45** | one table instead of two, one upsert, 7 reads lose a join, `last_event_id` and `event_id` go, `entity_attributes` loses `story_id`, replay deletes go; change-only `Updated` and the presence pass come in |
| C3: schema + model + events + projection | +50 to +65 | — | `relationships` and 2 indexes, kind/name checks, `relationship_id`, `EntityLink`, `link` on events, change-only link writes |
| C3: repository | +70 to +90 | — | the shared `SELECT` with display names, `load_entity_raw`, kind and name checks, `create_link_sync`/`update_link_sync` with the `both`, revival and orientation rules |
| C3: shared tool rules | +40 to +60 | — | `resolve_entity`, field parsing, `resolve_stats`/`apply_resolved_stats` and the savepoint |
| C3: `save_relationship` + settings | +55 to +75 | — | schema, description, handler, catalog spec, settings field and `serde(default)`, frontend row |
| C3: prompts, context, player edits, `roll_check` and `illustrate_scene` names | +28 to +43 | — | prompt rules, relationship lines, `recorded_content` and diff content, name matching in image generation; the stat lock goes |
| C3: removed `get_entities`, `entity_queried`, `object`/`location`/`campaign` starters, old pipeline settings | −102 to −117 | −9 to −14 | the tool and its plumbing, the transcript kind, 11 starter lines, the `attributes_enabled`/`dice_mode` strip and its test lines |
| **C3 total** | **+126 to +231** (incl. +5 to +8 imports) | **+150 to +200** | 12 test groups above |
| C4: schema + model + events + projection | +40 to +55 | — | `characters`, `CharacterFields`/`CharacterPatch`, event fields, change-only character writes |
| C4: repository + commands + readers | +45 to +60 | — | joins, `create_character_sync`, `update_character_sync` with diff content, validation, command arguments, image and context readers |
| C4: `save_character` + settings + context | +60 to +80 | — | schema, description, handler, catalog spec, settings key and frontend rows, the character line and header rule |
| C4: removed `create_entity` / `update_entity` / `adjust_entity_attribute` | −210 to −240 | — | 3 tool wrappers, labels, specs, builders, names, descriptions and schemas |
| **C4 net** | **−95 to −15** (gross +145 to +195) | **+60 to +85** | 7 test groups above |
| C5: settings + filter | +15 to +25 | — | `EntityVisibility`, filtering in `format_entity_context` |
| C5: removed Scoped | −45 to −60 | −40 to −60 | the scoped branch, `touched_entity_ids`, summary lines, `entities_touched_since`, `EntityContext`, the old select |
| C5: Context panel radios | +20 to +30 | — | the two-row fieldset and note |
| **C5 net** | **−10 to −5** | **+0 to +10** net | 3 test groups above |
| C6: types + API + store + command | +45 to +65 | +10 to +20 | `EntityLink`, labels, `list_story_attributes`, `attributesByEntity`, reloads, renames |
| C6: panel + stat queue | +82 to +120 | +15 to +25 | two tabs, character fields with change-only saving, Reveal toggle, relationship rows with stats, the label route, kind-aware attribute list |
| **C6 total** | **+127 to +185** | **+25 to +45** | |
| **Total** | **about +168 to +466 net** (gross about +715 to +975) | **about +262 to +397** | |
