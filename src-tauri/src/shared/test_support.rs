use rusqlite::Connection;
use serde_json::Value;

use crate::features::transcript::{repository, turns};

pub fn story(conn: &Connection, id: &str) -> String {
    story_with_settings(conn, id, serde_json::json!({}))
}

pub fn story_with_settings(conn: &Connection, id: &str, settings: Value) -> String {
    conn.execute(
        "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
         VALUES (?1, 'Story', 'now', 'now', ?2)",
        rusqlite::params![id, settings.to_string()],
    )
    .unwrap();
    id.to_string()
}

pub fn exchange(
    conn: &Connection,
    story_id: &str,
    mode: &str,
    action: &str,
    narration: Option<&str>,
) -> (String, String, Option<String>) {
    let turn_id = turns::create_turn(conn, story_id).unwrap();
    let action_id = repository::append_story_message(
        conn, story_id, "player", mode, action, None, Some(&turn_id),
    )
    .unwrap()
    .id;
    let narration_id = narration.map(|content| {
        repository::append_story_message(
            conn,
            story_id,
            "narrator",
            "generated",
            content,
            None,
            Some(&turn_id),
        )
        .unwrap()
        .id
    });
    (turn_id, action_id, narration_id)
}

pub fn record(
    conn: &Connection,
    story_id: &str,
    kind: &str,
    content: Option<&str>,
    payload: Value,
    target: Option<&str>,
    turn_id: Option<&str>,
) -> String {
    repository::append_entry(
        conn, story_id, kind, "hidden", content, &payload, target, turn_id,
    )
    .unwrap()
    .id
}
