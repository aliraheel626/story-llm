use rusqlite::OptionalExtension;

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

pub(super) fn image_paths_for_entry(
    conn: &rusqlite::Connection,
    entry_id: &str,
) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM image_assets WHERE entry_id = ?1")?;
    let rows = stmt.query_map([entry_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub(super) fn delete_for_entry(tx: &rusqlite::Transaction<'_>, entry_id: &str) -> AppResult<()> {
    tx.execute("DELETE FROM image_assets WHERE entry_id = ?1", [entry_id])?;
    Ok(())
}

pub(super) fn delete_asset_by_id(
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
