use crate::shared::error::AppResult;

pub fn cascade_entries(
    conn: &rusqlite::Connection,
    root_id: &str,
) -> AppResult<Vec<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE doomed(id) AS (
             SELECT ?1
             UNION
             SELECT ledger_entries.id FROM ledger_entries
             JOIN doomed ON ledger_entries.target_entry_id = doomed.id
         )
         SELECT ledger_entries.id, ledger_entries.kind, ledger_entries.payload_json
         FROM ledger_entries JOIN doomed ON doomed.id = ledger_entries.id",
    )?;
    let entries = stmt
        .query_map([root_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::ledger::{model::kind, repository};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn walks_all_descendants_without_changing_ledger_rows() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'Story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let root = repository::append_entry(
            &conn,
            "s",
            kind::NARRATION,
            "visible",
            Some("root"),
            &json!({}),
            None,
        )
        .unwrap();
        let child = repository::append_entry(
            &conn,
            "s",
            kind::ENTITY_UPDATED,
            "hidden",
            None,
            &json!({"entity_id":"entity"}),
            Some(&root.id),
        )
        .unwrap();
        let grandchild = repository::append_entry(
            &conn,
            "s",
            kind::DICEROLL,
            "hidden",
            None,
            &json!({"roll": 17}),
            Some(&child.id),
        )
        .unwrap();
        let sibling = repository::append_entry(
            &conn,
            "s",
            kind::IMAGE_GENERATED,
            "hidden",
            None,
            &json!({"asset_id":"image"}),
            Some(&root.id),
        )
        .unwrap();
        let unrelated = repository::append_entry(
            &conn,
            "s",
            kind::PLAYER_MESSAGE,
            "visible",
            Some("other"),
            &json!({}),
            None,
        )
        .unwrap();

        let entries: HashMap<String, (String, String)> = cascade_entries(&conn, &root.id)
            .unwrap()
            .into_iter()
            .map(|(id, kind, payload)| (id, (kind, payload)))
            .collect();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[&root.id].0, kind::NARRATION);
        assert_eq!(entries[&child.id].0, kind::ENTITY_UPDATED);
        assert_eq!(entries[&grandchild.id].1, r#"{"roll":17}"#);
        assert_eq!(entries[&sibling.id].1, r#"{"asset_id":"image"}"#);
        assert!(!entries.contains_key(&unrelated.id));
        assert!(cascade_entries(&conn, "missing").unwrap().is_empty());
        assert_eq!(
            repository::list_logical_entries(&conn, "s").unwrap().len(),
            5
        );
    }
}
