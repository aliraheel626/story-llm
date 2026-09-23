//! Every piece of English text sent to a model as an instruction lives here.
//! If you are changing what the narrator, title writer, or illustrator is told
//! to do, this is the file to edit.

use serde_json::{json, Value};

// Narrator

pub const TURN_MODES: &[&str] = &["do", "say", "story", "guide", "see", "continue"];

pub fn render_turn(mode: &str, content: &str) -> Option<String> {
    if !TURN_MODES.contains(&mode) {
        return None;
    }
    if matches!(mode, "see" | "continue") && content.is_empty() {
        return Some(format!("<{mode}/>"));
    }
    Some(format!("<{mode}>{content}</{mode}>"))
}

const NARRATOR_SYSTEM_PROMPT_BASE: &str = r#"# Role

You are the narrator of an interactive story.

# Narration

- Continue the scene in vivid, literary prose that follows naturally from what has already happened and from the player's latest action.
- Match the established tone, tense, and style unless the current author's note directs otherwise.
- Always narrate the player's actions and perceptions in the second person ("you"). Other characters stay in the third person.
- Never speak as the player, never break the fourth wall, and never add meta-commentary, author's notes, or content outside the story itself.

# Content

This is private, single-player fiction the player is authoring for themselves — no one else ever sees it.

- Violence, combat, crime, death, and morally gray or dark themes are ordinary genre content here, not something to avoid, soften, or redirect away from. Narrate them with the same craft as any other scene.
- Never break narration to refuse, warn, moralize, or ask the player to reconsider. If an action would have serious in-fiction consequences, let those consequences play out in the story itself.
- The only real-world line: don't render content that would be illegal or genuinely harmful outside the fiction (e.g. actionable real-world instructions for violence, or sexual content involving minors). Ordinary dramatic violence and dark themes are not that.

# Input format

Each turn arrives as tagged input. These tags are input markup. Never reproduce them in output.

## Player turns

- `<do>` is an attempted action.
- `<say>` is spoken words.
- `<story>` is an author-written passage to complete.
- `<guide>` is out-of-character steering.
- `<continue/>` means keep going.
- `<see>` is a request to see something. Always call `illustrate_scene`, whatever the subject, for the named subject or, when none is named, the current scene. Never skip it. Make it a real tool call through the tool-calling interface; never write the call out as text, and write no other prose. On your own initiative, illustrate only genuinely striking moments.

## Context blocks

- `<entities>` is authoritative, and user overrides win.
- `<author_note>` contains the author's current standing guidance. Follow it on every turn while present, including tone, style, pacing, and scene constraints even when they differ from earlier narration. Keep the rules above, authoritative story facts and rolls, and the player's latest action intact. Never quote or mention the note in your reply.
- `<additional_instructions>` contains extra guidance for this turn, such as tool availability and when to use those tools.

## Records in history

- `[Authoritative story event: …]` is a system record of something that already happened, such as a dice roll, an entity change, or a generated image. Treat it as fact. It is not a reply, so never write one yourself.
- `[Authoritative context summary]` replaces older history that was compacted. Treat it as fact.

Your replies are only narration or real tool calls.
"#;

/// The narrator's fixed system prompt. Story-specific context is prepended to
/// the player's action instead of being baked into this value.
pub fn narrator_system_prompt() -> String {
    NARRATOR_SYSTEM_PROMPT_BASE.trim_end().to_string()
}

pub const IMAGE_TOOL_AVAILABLE_INSTRUCTION: &str = "You have an illustrate_scene tool: always call it when the player sends <see>, and otherwise only for a genuinely striking moment.";

pub const ENTITY_CONTEXT_HEADER: &str =
    "Current entity state is authoritative. User overrides take precedence over inferred updates.";
pub const ROLL_CHECK_AVAILABLE_INSTRUCTION: &str = "For a genuinely uncertain outcome, call roll_check before narrating the result. With no factors it defaults to 50% unless you provide chance_percent. For a check based on registered attributes, select one acting entity-attribute pair or two opposing pairs; the backend reads their current values and calculates the chance. Use get_entities to find IDs and attribute names when that tool is available. Never invent attribute values or pass chance_percent together with factors. Do not roll routine or certain actions.";

// Title generation

pub const TITLE_SYSTEM_PROMPT: &str =
    "You name interactive stories. Given the opening of a story, give it \
a short, evocative title of 2 to 5 words that fits its tone and language. Respond with the \
structured output only.";

// Compaction

pub const COMPACTION_SUMMARY_SYSTEM_PROMPT: &str = "Summarize an interactive story's older context. Preserve concrete facts, promises, relationships, unresolved plot threads, dice-roll outcomes, entity-relevant details, and all guide/story directives expressed by tagged input turns. Do not invent events. Return structured output only.";

// Tool specs

pub const ILLUSTRATE_SCENE_TOOL_NAME: &str = "illustrate_scene";
pub const ILLUSTRATE_SCENE_DESCRIPTION: &str =
    "Generate a scene image. Always call this when the player sends <see>, whatever the \
     subject: that is an explicit request, so never skip it or answer in prose. Invoke it as a real \
     tool call; never write the call out as text. When you choose \
     to illustrate on your own, use it sparingly: reserve it for a genuinely striking visual \
     moment (a new place revealed, a character's first appearance, a dramatic turn worth \
     seeing). Write a vivid, concrete visual description of the subject as it appears in the \
     story: subject, setting, composition, lighting. Do not mention art style or medium; \
     that's applied separately.";

pub fn illustrate_scene_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "description": {"type": "string", "description": "A vivid, concrete visual description of the scene's subject, setting, composition, and lighting."},
            "character_ids": {"type": "array", "items": {"type": "string"}, "description": "Ids of characters visible in the scene, from get_entities."}
        },
        "required": ["description"]
    })
}

pub const ROLL_CHECK_TOOL_NAME: &str = "roll_check";
pub const ROLL_CHECK_DESCRIPTION: &str =
    "Resolve a genuinely uncertain action. With zero factors, chance_percent is optional and \
     defaults to 50. For one factor, identify the acting entity_id and attribute_name; for two, \
     put the acting pair first and the opposing pair second. The backend reads stored attribute \
     values, normalizes each by its registered min/max, and calculates chance_percent as \
     round(50 + 50 * (actor_normalized - opponent_normalized)); a single factor faces a neutral \
     opponent at 0.5. Do not pass chance_percent with factors, and do not invent entity IDs or \
     values. The tool returns the draw and success or failure.";

pub fn roll_check_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "chance_percent": {"type": "integer", "minimum": 0, "maximum": 100, "description": "Optional narrator-estimated chance when no factors are given; defaults to 50%. Must be omitted when factors are present."},
            "reason": {"type": "string", "description": "A short description of the uncertain action and why it needs a roll."},
            "factors": {
                "type": "array", "maxItems": 2,
                "description": "Zero, one acting, or two acting-then-opposing registered entity attributes. Values are fetched by the backend; never supply numbers here.",
                "items": {
                    "type": "object",
                    "properties": {
                        "entity_id": {"type": "string", "description": "Entity id in this story, preferably from get_entities."},
                        "attribute_name": {"type": "string", "description": "Name of an attribute currently set on the entity."}
                    },
                    "required": ["entity_id", "attribute_name"],
                    "additionalProperties": false
                }
            }
        },
        "additionalProperties": false
    })
}

pub const GET_ENTITIES_TOOL_NAME: &str = "get_entities";
pub const GET_ENTITIES_DESCRIPTION: &str =
    "List known entities (characters, objects, locations) and their current attribute values. \
     Use this to check who or what is present before narrating or changing entity state.";

pub fn get_entities_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "description": "Filter by kind: character, object, location, relationship, or campaign."},
            "name": {"type": "string", "description": "Filter to an exact (case-insensitive) name match."}
        }
    })
}

pub const CREATE_ENTITY_TOOL_NAME: &str = "create_entity";
pub const CREATE_ENTITY_DESCRIPTION: &str =
    "Introduce a new entity (character, object, or location) the story just established. \
     Idempotent by name — calling this for an entity that already exists just returns it.";

pub fn create_entity_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "description": "character, object, location, relationship, or campaign."},
            "name": {"type": "string", "description": "The entity's name, exactly as it should appear in the story."},
            "appearance_anchor": {"type": "string", "description": "A short, stable visual description to keep the entity consistent."}
        },
        "required": ["kind", "name"]
    })
}

pub const UPDATE_ENTITY_TOOL_NAME: &str = "update_entity";
pub const UPDATE_ENTITY_DESCRIPTION: &str =
    "Rename an entity or update its appearance description. Use a known entity id; look it up first when the lookup tool is available.";

pub fn update_entity_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {"type": "string", "description": "Entity id from get_entities/create_entity."},
            "name": {"type": "string", "description": "The entity's (possibly unchanged) name."},
            "appearance_anchor": {"type": "string", "description": "The entity's (possibly unchanged) appearance description."}
        },
        "required": ["id", "name"]
    })
}

pub const ADJUST_ENTITY_ATTRIBUTE_TOOL_NAME: &str = "adjust_entity_attribute";
pub const ADJUST_ENTITY_ATTRIBUTE_DESCRIPTION: &str =
    "Change an entity's attribute by a delta implied by what just happened (an injury, growing \
     trust, a depleted resource). Most changes are minor; only set dramatic for a genuinely \
     major, story-changing swing.";

pub fn adjust_entity_attribute_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "entity_id": {"type": "string", "description": "A known entity id from available context or an enabled lookup/create tool. The player character is named \"You\"."},
            "attribute": {"type": "string", "description": "Attribute name, e.g. \"Trust\"."},
            "delta": {"type": "number", "description": "Positive or negative change, on the attribute's own scale."},
            "dramatic": {"type": "boolean", "description": "True only for a major, story-changing swing."},
            "reason": {"type": "string", "description": "Why this changed, for the audit log."}
        },
        "required": ["entity_id", "attribute", "delta", "reason"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_renderer_uses_one_canonical_tagged_form() {
        assert_eq!(
            render_turn("do", "Open it."),
            Some("<do>Open it.</do>".into())
        );
        assert_eq!(render_turn("say", "Hello"), Some("<say>Hello</say>".into()));
        assert_eq!(render_turn("continue", ""), Some("<continue/>".into()));
        assert_eq!(render_turn("see", ""), Some("<see/>".into()));
        assert_eq!(render_turn("unknown", "x"), None);
    }

    #[test]
    fn narrator_prompt_follows_current_author_note_without_exposing_it() {
        let prompt = narrator_system_prompt();
        assert!(prompt.contains("unless the current author's note directs otherwise"));
        assert!(prompt.contains("Follow it on every turn while present"));
        assert!(prompt.contains("Never quote or mention the note in your reply"));
    }

    #[test]
    fn roll_check_schema_accepts_optional_chance_or_two_attribute_references() {
        let schema = roll_check_schema();
        assert!(schema.get("required").is_none());
        assert_eq!(schema["properties"]["chance_percent"]["minimum"], 0);
        assert_eq!(schema["properties"]["chance_percent"]["maximum"], 100);
        assert_eq!(schema["properties"]["factors"]["maxItems"], 2);
        assert_eq!(
            schema["properties"]["factors"]["items"]["required"],
            json!(["entity_id", "attribute_name"])
        );
        assert!(schema["properties"].get("attribute").is_none());
    }
}
