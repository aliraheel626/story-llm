//! Every piece of English text sent to a model as an instruction lives here.
//! If you are changing what the narrator, title writer, or illustrator is told
//! to do, this is the file to edit.

use serde_json::{json, Value};

use crate::features::dicerolls::model::DiceMode;

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
- Match the established tone, tense, and style.
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
- `<author_note>` is the author's standing direction.
- `<rolls>` contains resolved outcomes that must never be contradicted.
- `<dice_mode>` controls when `roll_check` may be called.

## Records in history

- `[Authoritative story event: …]` is a system record of something that already happened, such as a dice roll, an entity change, or a generated image. Treat it as fact. It is not a reply, so never write one yourself.
- `[Authoritative context summary]` replaces older history that was compacted. Treat it as fact.

Your replies are only narration or real tool calls.
"#;

/// The narrator's system prompt, written as markdown. The author note is the
/// only per-story part; it goes last, wrapped in its tag so a note that itself
/// contains markdown headings can't be mistaken for the prompt's own structure.
pub fn narrator_system_prompt(author_note: Option<&str>) -> String {
    let mut prompt = NARRATOR_SYSTEM_PROMPT_BASE.trim_end().to_string();
    if let Some(note) = author_note.map(str::trim).filter(|note| !note.is_empty()) {
        prompt.push_str("\n\n# Author's note\n\n<author_note>");
        prompt.push_str(note);
        prompt.push_str("</author_note>");
    }
    prompt
}

pub const IMAGE_TOOL_AVAILABLE_INSTRUCTION: &str = "You have an illustrate_scene tool: always call it when the player sends <see>, and otherwise only for a genuinely striking moment.";

pub const ENTITY_CONTEXT_HEADER: &str =
    "Current entity state is authoritative. User overrides take precedence over inferred updates.";
pub const ENTITY_TOOLS_AVAILABLE_PREFIX: &str = "You have tools to check, create, and update entities and their attributes as the story unfolds — use them to keep the world consistent.";

pub fn dice_mode_instruction(dice_mode: DiceMode) -> &'static str {
    match dice_mode {
        DiceMode::Always => {
            "Call the roll_check tool for every meaningful action before narrating its outcome."
        }
        DiceMode::Classifier => {
            "Call the roll_check tool only when the outcome is genuinely uncertain — routine or \
             clearly one-sided actions don't need it."
        }
        DiceMode::Never => "Do not call the roll_check tool; narrate outcomes purely from context.",
    }
}

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
    "Roll the dice for an uncertain action. Resolves the player's relevant attribute against an \
     optional opposing entity/attribute and returns the outcome. Call this before narrating the \
     result of any action whose success is genuinely in doubt.";

pub fn roll_check_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "attribute": {"type": "string", "description": "The player's attribute this action draws on, e.g. \"Stealth\"."},
            "target_entity_id": {"type": "string", "description": "Id of the opposing entity, from get_entities, if any."},
            "target_attribute": {"type": "string", "description": "The opposing entity's attribute, if target_entity_id is given."},
            "modifier": {"type": "number", "description": "Situational adjustment to success probability, e.g. 0.1 for +10%."}
        },
        "required": ["attribute"]
    })
}

pub const GET_ENTITIES_TOOL_NAME: &str = "get_entities";
pub const GET_ENTITIES_DESCRIPTION: &str =
    "List known entities (characters, objects, locations) and their current attribute values. \
     Use this to check who or what is present before narrating, rolling, or adjusting state.";

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
    "Rename an entity or update its appearance description. Look it up with get_entities first.";

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
            "entity_id": {"type": "string", "description": "Entity id from get_entities/create_entity. Use \"You\" for the player via get_entities first."},
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
    fn narrator_prompt_bakes_the_selected_note() {
        let prompt = narrator_system_prompt(Some("Keep it terse."));
        assert!(prompt.contains("<author_note>Keep it terse.</author_note>"));
        assert!(prompt.contains("Never reproduce them in output."));
        assert_eq!(prompt, narrator_system_prompt(Some("Keep it terse.")));
        assert_ne!(prompt, narrator_system_prompt(Some("Use long sentences.")));
    }
}
