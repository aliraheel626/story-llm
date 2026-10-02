//! Narrator tool modules and shared activity labels.

pub(crate) mod adjust_entity_attribute;
pub(crate) mod create_entity;
pub(crate) mod illustrate_scene;
pub(crate) mod roll_check;
pub(crate) mod save_relationship;
mod shared;
pub(crate) mod update_entity;

use rig_agent::tool::ToolExecutionError;

use crate::shared::error::AppError;

use super::catalog;

fn to_tool_error(e: AppError) -> ToolExecutionError {
    match e {
        AppError::Invalid(message) | AppError::NotFound(message) => {
            ToolExecutionError::invalid_args(message)
        }
        _ => ToolExecutionError::other(e.to_string()),
    }
}

/// Tool name + args → a friendly, generic activity label for the frontend.
pub fn friendly_tool_label(tool_name: &str, args_json: &str) -> String {
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
    catalog::TOOLS
        .iter()
        .find(|spec| spec.name == tool_name)
        .map_or_else(
            || format!("Running {tool_name}…"),
            |spec| (spec.label)(&args),
        )
}

#[cfg(test)]
pub(super) mod test_support {
    use std::sync::Arc;

    use serde_json::json;

    use crate::features::transcript::repository::append_entry;
    use crate::features::turn::{TurnGate, TurnTx};
    use crate::shared::db::Pool;

    pub fn fixture() -> (Pool, Arc<TurnTx>, String, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        let story_id = uuid::Uuid::new_v4().to_string();
        conn.execute("INSERT INTO stories(id, title, created_at, updated_at, settings_json) VALUES (?1, 't', 'now', 'now', '{}')", [&story_id]).unwrap();
        let turn_id = crate::features::transcript::turns::create_turn(&conn, &story_id).unwrap();
        let target = append_entry(
            &conn,
            &story_id,
            crate::features::transcript::model::kind::NARRATION,
            "visible",
            Some("scene"),
            &json!({}),
            None,
            Some(&turn_id),
        )
        .unwrap()
        .id;
        drop(conn);
        let turn = TurnTx::begin(&pool, &TurnGate::default(), &story_id).unwrap();
        (pool, turn, target, turn_id)
    }
}

#[cfg(test)]
mod turn_tests {
    use super::super::catalog::{self, ToolAvailability, ToolDeps};
    use super::{friendly_tool_label, test_support::fixture};
    use crate::features::stories::settings::NarratorToolSettings;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn labels_and_catalog_keep_existing_contract() {
        assert_eq!(
            friendly_tool_label("create_entity", r#"{"name":"Mira"}"#),
            "Introducing Mira…"
        );
        assert_eq!(
            friendly_tool_label("roll_check", r#"{"reason":"escaping"}"#),
            "Rolling for escaping…"
        );
        assert_eq!(friendly_tool_label("unknown", "{}"), "Running unknown…");
        let settings = NarratorToolSettings::default();
        let specs = catalog::enabled(&ToolAvailability {
            settings: &settings,
            image_enabled: false,
            illustrate: false,
        });
        assert_eq!(specs.len(), 5);
        assert_eq!(specs[0].name, "roll_check");
        assert_eq!(specs.iter().map(|spec| spec.name).collect::<Vec<_>>(),
            ["roll_check", "create_entity", "update_entity", "adjust_entity_attribute", "save_relationship"]);
        let (_pool, turn, target, turn_id) = fixture();
        let deps = ToolDeps {
            turn: Some(turn),
            target_entry_id: Some(target),
            turn_id: Some(turn_id),
            embedding_api_key: String::new(),
            image_requests: Arc::new(Mutex::new(Vec::new())),
        };
        assert_eq!((specs[0].build)(&deps).name(), "roll_check");
    }
}
