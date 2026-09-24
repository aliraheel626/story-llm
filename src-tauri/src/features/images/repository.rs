use rusqlite::OptionalExtension;

use crate::features::ledger::model::kind as ledger_kind;
use crate::shared::error::AppResult;

use super::model::StoryImage;

fn row_to_image(row: &rusqlite::Row) -> rusqlite::Result<StoryImage> {
    Ok(StoryImage {
        id: row.get(0)?,
        entry_id: row.get(1)?,
        path: row.get(2)?,
        prompt: row.get(3)?,
        created_at: row.get(4)?,
    })
}

pub(super) fn list_for_story(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Vec<StoryImage>> {
    let mut stmt = conn.prepare(
        "SELECT image_assets.id, image_assets.entry_id, image_assets.path, image_assets.prompt, image_assets.created_at
         FROM image_assets JOIN ledger_entries ON ledger_entries.id = image_assets.entry_id
         WHERE ledger_entries.story_id = ?1 ORDER BY image_assets.created_at ASC",
    )?;
    let rows = stmt.query_map([story_id], row_to_image)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub(super) fn insert_asset(conn: &rusqlite::Connection, image: &StoryImage) -> AppResult<()> {
    conn.execute(
        "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            image.id,
            image.entry_id,
            image.path,
            image.prompt,
            image.created_at
        ],
    )?;
    Ok(())
}

pub fn image_paths_for_entry(
    conn: &rusqlite::Connection,
    entry_id: &str,
) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM image_assets WHERE entry_id = ?1")?;
    let rows = stmt.query_map([entry_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn image_paths_for_story(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT image_assets.path FROM image_assets
         JOIN ledger_entries ON ledger_entries.id = image_assets.entry_id
         WHERE ledger_entries.story_id = ?1",
    )?;
    let paths = stmt
        .query_map([story_id], |row| row.get(0))?
        .filter_map(Result::ok)
        .collect();
    Ok(paths)
}

fn delete_for_entry(tx: &rusqlite::Transaction<'_>, entry_id: &str) -> AppResult<()> {
    tx.execute("DELETE FROM image_assets WHERE entry_id = ?1", [entry_id])?;
    Ok(())
}

pub fn detach_from_entry(tx: &rusqlite::Transaction<'_>, entry_id: &str) -> AppResult<Vec<String>> {
    let paths = image_paths_for_entry(tx, entry_id)?;
    tx.execute(
        "DELETE FROM ledger_entries WHERE kind = ?1 AND (
             target_entry_id = ?2 OR
             CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.asset_id') END
                 IN (SELECT id FROM image_assets WHERE entry_id = ?2)
         )",
        rusqlite::params![ledger_kind::IMAGE_GENERATED, entry_id],
    )?;
    delete_for_entry(tx, entry_id)?;
    Ok(paths)
}

/// Call on the transactional connection while collecting cascade effects.
pub fn delete_asset_by_id(
    conn: &rusqlite::Connection,
    asset_id: &str,
) -> AppResult<Option<String>> {
    let path = conn
        .query_row(
            "SELECT path FROM image_assets WHERE id = ?1",
            [asset_id],
            |row| row.get(0),
        )
        .optional()?;
    conn.execute("DELETE FROM image_assets WHERE id = ?1", [asset_id])?;
    Ok(path)
}

/// Delete files only after the transaction that detached them has committed.
pub fn delete_assets(paths: &[String]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detaching_an_entry_removes_only_its_images_and_events() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"CREATE TABLE image_assets(id TEXT PRIMARY KEY, entry_id TEXT, path TEXT);
             CREATE TABLE ledger_entries(id TEXT PRIMARY KEY, kind TEXT, target_entry_id TEXT, payload_json TEXT);
             INSERT INTO image_assets VALUES ('asset-1', 'entry-1', 'first.png');
             INSERT INTO image_assets VALUES ('asset-2', 'entry-2', 'second.png');
             INSERT INTO ledger_entries VALUES ('event-1', 'image_generated', 'entry-1', '{"asset_id":"asset-1"}');
             INSERT INTO ledger_entries VALUES ('event-2', 'image_generated', 'entry-2', '{"asset_id":"asset-2"}');
             INSERT INTO ledger_entries VALUES ('event-3', 'content_edited', 'entry-1', '{}');
             INSERT INTO ledger_entries VALUES ('event-4', 'image_generated', 'see-action', '{"asset_id":"asset-1"}');
             INSERT INTO ledger_entries VALUES ('event-5', 'image_generated', 'other-see-action', '{"asset_id":"asset-2"}');"#,
        )
        .unwrap();

        let tx = conn.transaction().unwrap();
        assert_eq!(
            detach_from_entry(&tx, "entry-1").unwrap(),
            vec!["first.png"]
        );
        assert_eq!(
            image_paths_for_entry(&tx, "entry-1").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            image_paths_for_entry(&tx, "entry-2").unwrap(),
            vec!["second.png"]
        );
        let event_ids: Vec<String> = tx
            .prepare("SELECT id FROM ledger_entries ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(event_ids, vec!["event-2", "event-3", "event-5"]);
        assert_eq!(
            delete_asset_by_id(&tx, "asset-2").unwrap(),
            Some("second.png".into())
        );
        assert_eq!(delete_asset_by_id(&tx, "asset-2").unwrap(), None);
        tx.commit().unwrap();
    }
}
