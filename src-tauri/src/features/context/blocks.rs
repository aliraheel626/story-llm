//! Ordered per-message context assembly for narrator requests.

use std::collections::HashMap;

use crate::features::entities::{
    self,
    model::{Entity, EntityAttributeValue, CHARACTER, RELATIONSHIP},
};
use crate::features::stories::settings::{ContextSettings, EntityVisibility};
use crate::prompts;
use crate::shared::error::AppResult;

pub(crate) struct Inputs<'a> {
    pub conn: &'a rusqlite::Connection,
    pub story_id: &'a str,
    pub context: &'a ContextSettings,
    pub rejected_reply: Option<&'a str>,
}

pub(crate) struct ContextPlan {
    pub live: String,
    pub full: String,
}

fn format_entity_context(
    data: &[Entity],
    attributes: &HashMap<String, Vec<EntityAttributeValue>>,
    visibility: EntityVisibility,
) -> String {
    let mut lines = Vec::new();
    for entity in data.iter().filter(|entity| match entity.kind.as_str() {
        CHARACTER => visibility.character,
        RELATIONSHIP => visibility.relationship,
        _ => false,
    }) {
        let mut details = String::new();
        if let Some(link) = &entity.link {
            if let Some(text) = &link.description {
                details.push_str(&format!("; description: {text}"));
            }
        } else {
            for (label, value) in [
                ("known to the player as", &entity.character.known_as),
                ("location", &entity.character.location),
                ("outfit", &entity.character.outfit),
                ("gender", &entity.character.gender),
                ("age", &entity.character.age),
                ("role", &entity.character.role),
                ("appearance", &entity.character.appearance_anchor),
            ] {
                if let Some(text) = value {
                    details.push_str(&format!("; {label}: {text}"));
                }
            }
        }
        let attributes = match attributes.get(&entity.id) {
            Some(attrs) if !attrs.is_empty() => format!(
                "; attributes: {}",
                attrs
                    .iter()
                    .map(|attribute| format!("{}={}", attribute.canonical_name, attribute.value))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => String::new(),
        };
        lines.push(format!(
            "- {} ({}){details}{attributes}",
            entity.name, entity.kind
        ));
    }
    if lines.is_empty() {
        String::new()
    } else {
        format!("<entities>\n{}\n{}\n</entities>", prompts::ENTITY_CONTEXT_HEADER, lines.join("\n"))
    }
}

pub(crate) fn combine_context_blocks(parts: &[String]) -> String {
    parts
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(crate) fn build_message_context(inputs: &Inputs<'_>) -> AppResult<ContextPlan> {
    let (entities, attributes) = if !inputs.context.entity_kinds.character && !inputs.context.entity_kinds.relationship {
        (Vec::new(), HashMap::new())
    } else {
        let entities = entities::list_entities_sync(inputs.conn, inputs.story_id, None)?;
        let ids = entities
            .iter()
            .map(|entity| entity.id.as_str())
            .collect::<Vec<_>>();
        let attributes = entities::attributes::list_entity_attributes_for_entities_sync(
            inputs.conn,
            inputs.story_id,
            &ids,
        )?;
        (entities, attributes)
    };
    let entities_full = format_entity_context(&entities, &attributes, inputs.context.entity_kinds);
    let note = inputs.context.author_note.trim();
    let author_note = if inputs.context.author_note_enabled && !note.is_empty() {
        format!("<author_note>{note}</author_note>")
    } else {
        String::new()
    };
    let full = combine_context_blocks(&[entities_full, author_note]);
    let retry = inputs
        .rejected_reply
        .map(prompts::retry_instruction)
        .unwrap_or_default();
    let live = combine_context_blocks(&[full.clone(), retry]);
    Ok(ContextPlan { live, full })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{
        entities,
        turn::TurnTx,
    };
    use crate::shared::db::Pool;
    use serde_json::json;

    fn story() -> Pool {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story_with_settings(
            &conn, "s", json!({"context": {"author_note":"Keep it terse."}}),
        );
        entities::create_entity_with_id_sync(
            &conn,
            "bob",
            "s",
            "character",
            "Bob",
            Some("a red cloak"),
            "test",
            None,
            None,
        )
        .unwrap();
        drop(conn);
        pool
    }

    #[tokio::test]
    async fn pipeline_builds_all_blocks_in_fixed_order() {
        let pool = story();
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        let plan = turn
            .with(|conn| {
                let context = crate::features::stories::settings::read_context_settings(conn, "s")?;
                build_message_context(&Inputs {
                    conn,
                    story_id: "s",
                    context: &context,
                    rejected_reply: None,
                })
            })
            .await
            .unwrap();

        assert!(plan
            .live
            .contains("- Bob (character); appearance: a red cloak"));
        let entities = plan.live.find("<entities>").unwrap();
        let note = plan.live.find("<author_note>").unwrap();
        assert!(entities < note);
        assert!(plan.full.contains("<entities>"));
        assert_eq!(plan.full, plan.live);
    }

    #[tokio::test]
    async fn context_sees_uncommitted_story_and_entity_changes() {
        let pool = story();
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        turn.with(|conn| {
            conn.execute(
                "UPDATE stories SET settings_json = ?1 WHERE id = 's'",
                [json!({"context": {"author_note":"A new direction.", "author_note_enabled":true}}).to_string()],
            )?;
            entities::create_entity_with_id_sync(
                conn, "alice", "s", "character", "Alice", Some("a blue coat"),
                "test", None, None,
            )?;
            Ok(())
        }).await.unwrap();
        let plan = turn
            .with(|conn| {
                let context = crate::features::stories::settings::read_context_settings(conn, "s")?;
                build_message_context(&Inputs {
                    conn,
                    story_id: "s",
                    context: &context,
                    rejected_reply: None,
                })
            })
            .await
            .unwrap();

        assert!(plan
            .full
            .contains("Alice (character); appearance: a blue coat"));
        assert!(plan
            .live
            .contains("<author_note>A new direction.</author_note>"));
        assert!(plan
            .live
            .contains("Bob (character); appearance: a red cloak"));
        assert!(plan
            .live
            .contains("Alice (character); appearance: a blue coat"));
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn entity_snapshot_reads_uncommitted_attributes_on_turn_connection() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story(&conn, "s");
        drop(conn);
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        turn.with(|conn| {
            entities::create_entity_with_id_sync(
                conn,
                "alice",
                "s",
                "character",
                "Alice",
                Some("blue coat"),
                "test",
                None,
                None,
            )?;
            let attribute_id: String = conn.query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Accuracy'",
                [],
                |row| row.get(0),
            )?;
            entities::attributes::set_entity_attribute_sync(
                conn,
                "s",
                "alice",
                &attribute_id,
                7.0,
            )?;
            let snapshot = entities::list_entities_sync(conn, "s", None)?;
            let alice = snapshot.iter().find(|entity| entity.id == "alice").unwrap();
            let attributes = entities::attributes::list_entity_attributes_for_entities_sync(
                conn,
                "s",
                &["alice"],
            )?;
            let alice_attributes = &attributes["alice"];
            assert_eq!(snapshot.len(), 1);
            assert_eq!(alice.id, "alice");
            assert_eq!(alice.name, "Alice");
            assert_eq!(alice.character.appearance_anchor.as_deref(), Some("blue coat"));
            assert_eq!(alice_attributes.len(), 1);
            assert_eq!(alice_attributes[0].canonical_name, "Accuracy");
            assert_eq!(alice_attributes[0].value, 7.0);
            let context = ContextSettings {
                entity_kinds: EntityVisibility::default(),
                author_note_enabled: false,
                author_note: String::new(),
            };
            let plan = build_message_context(&Inputs {
                conn,
                story_id: "s",
                context: &context,
                rejected_reply: None,
            })?;
            assert!(plan
                .full
                .contains("Alice (character); appearance: blue coat; attributes: Accuracy=7"));
            assert_eq!(plan.full, plan.live);
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn hidden_kinds_and_muted_note_leave_no_context() {
        let pool = story();
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        let plan = turn
            .with(|conn| {
                let context = ContextSettings {
                    entity_kinds: EntityVisibility { character: false, relationship: false },
                    author_note_enabled: false,
                    author_note: "Keep it terse.".into(),
                };
                build_message_context(&Inputs {
                    conn,
                    story_id: "s",
                    context: &context,
                    rejected_reply: None,
                })
            })
            .await
            .unwrap();
        assert!(plan.live.is_empty());
        assert!(plan.full.is_empty());
        turn.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn retry_adds_rejected_text_only_to_live_context_last() {
        let pool = story();
        let turn = TurnTx::begin(&pool, &Default::default(), "s").unwrap();
        turn.with(|conn| {
            let context = crate::features::stories::settings::read_context_settings(conn, "s")?;
            let base = Inputs {
                conn,
                story_id: "s",
                context: &context,
                rejected_reply: Some("Rejected narration."),
            };
            let retry = build_message_context(&base)?;
            assert!(retry
                .live
                .contains("<rejected_reply>Rejected narration.</rejected_reply>"));
            assert!(retry.live.find("</author_note>").unwrap() < retry.live.find("<retry>").unwrap());
            assert!(!retry.full.contains("<retry>"));
            let normal = build_message_context(&Inputs {
                rejected_reply: None,
                ..base
            })?;
            assert!(!normal.live.contains("<retry>"));
            assert_eq!(normal.full, retry.full);
            Ok(())
        })
        .await
        .unwrap();
        turn.rollback().await.unwrap();
    }

    #[test]
    fn character_facts_render_in_order_with_true_name_and_perceived_title() {
        use crate::features::entities::model::CharacterFields;
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story(&conn, "s");
        let kael = entities::repository::create_character_sync(&conn, "s", "Kael", CharacterFields {
            known_as: Some("the hooded stranger".into()), appearance_anchor: Some("scarred lip".into()),
            gender: Some("female".into()), age: Some("34".into()), role: Some("smuggler".into()),
            location: Some("the Rusty Anchor".into()), outfit: Some("grey cloak".into()),
        }, "narrator_tool", None, None).unwrap();
        let health = entities::registry::find_exact_match(&conn, "Health").unwrap().unwrap();
        entities::attributes::set_entity_attribute_sync(&conn, "s", &kael.id, &health.id, 7.0).unwrap();
        let data = entities::list_entities_sync(&conn, "s", None).unwrap();
        let attrs = entities::attributes::list_entity_attributes_for_entities_sync(&conn, "s", &[&kael.id]).unwrap();
        let text = format_entity_context(&data, &attrs, EntityVisibility::default());
        assert!(text.contains("- Kael (character); known to the player as: the hooded stranger; location: the Rusty Anchor; outfit: grey cloak; gender: female; age: 34; role: smuggler; appearance: scarred lip; attributes: Health=7"));
        assert!(text.contains("Names here are true names."));
    }

    #[test]
    fn relationship_context_has_current_names_directions_description_and_stats() {
        use crate::features::entities::model::EntityLink;
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        crate::shared::test_support::story(&conn, "s");
        for name in ["Mira", "You", "Varro"] {
            entities::create_entity_with_id_sync(&conn, name, "s", "character", name, None, "test", None, None).unwrap();
        }
        let directed = entities::repository::create_link_sync(&conn, "s", EntityLink {
            from_id: "Mira".into(), to_id: "You".into(), label: "estranged sister".into(),
            direction: "one_way".into(), description: Some("Still resents your departure".into()),
        }, "test", None, None).unwrap();
        entities::repository::create_link_sync(&conn, "s", EntityLink {
            from_id: "Mira".into(), to_id: "Varro".into(), label: "siblings".into(),
            direction: "both".into(), description: None,
        }, "test", None, None).unwrap();
        for (name, value) in [("Affection", 8.0), ("Trust", -2.0)] {
            let definition = entities::registry::find_exact_match(&conn, name).unwrap().unwrap();
            entities::attributes::set_entity_attribute_sync(&conn, "s", &directed.id, &definition.id, value).unwrap();
        }
        let data = entities::list_entities_sync(&conn, "s", None).unwrap();
        let ids = data.iter().map(|entity| entity.id.as_str()).collect::<Vec<_>>();
        let attrs = entities::attributes::list_entity_attributes_for_entities_sync(&conn, "s", &ids).unwrap();
        let text = format_entity_context(&data, &attrs, EntityVisibility::default());
        assert!(text.starts_with(&format!("<entities>\n{}", prompts::ENTITY_CONTEXT_HEADER)));
        assert!(text.contains("- Mira → You: estranged sister (relationship); description: Still resents your departure; attributes: Affection=8, Trust=-2"));
        assert!(text.contains("- Mira ↔ Varro: siblings (relationship)"));
        for (character, relationship) in [(true, false), (false, true), (false, false)] {
            let context = ContextSettings {
                entity_kinds: EntityVisibility { character, relationship },
                author_note_enabled: false,
                ..Default::default()
            };
            let plan = build_message_context(&Inputs {
                conn: &conn, story_id: "s", context: &context, rejected_reply: None,
            }).unwrap();
            assert_eq!(plan.full, plan.live);
            assert_eq!(plan.live.lines().any(|line| line.starts_with("- ") && line.contains("(character)")), character);
            assert_eq!(plan.live.lines().any(|line| line.starts_with("- ") && line.contains(" → ")), relationship);
            assert_eq!(plan.live.contains("\n- Mira ↔ Varro: siblings (relationship)"), relationship);
            assert_eq!(plan.live.contains("<entities>"), character || relationship);
            if relationship {
                assert!(plan.live.contains("- Mira → You: estranged sister (relationship); description: Still resents your departure; attributes: Affection=8, Trust=-2"));
            }
        }
        assert_eq!(format_entity_context(&[], &HashMap::new(), EntityVisibility::default()), "");
    }
}
