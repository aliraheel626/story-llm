use crate::ai::HistoryTurn;
use crate::shared::db::Pool;
use crate::shared::error::AppResult;

const HISTORY_LIMIT: i64 = 60;

/// Loads recent passage history oldest-first, excluding an optional target
/// sequence used by Retry and Swipe.
pub(super) fn load_history(
    pool: &Pool,
    branch_id: &str,
    before_seq: Option<i64>,
) -> AppResult<Vec<HistoryTurn>> {
    let conn = pool.get()?;
    let mut rows_out: Vec<(String, HistoryTurn)> = Vec::new();
    const SELECT: &str = "SELECT passages.role, passages.input_mode, passages.content,
        (SELECT images.prompt FROM images WHERE images.passage_id = passages.id ORDER BY images.created_at DESC LIMIT 1)
        FROM passages WHERE passages.branch_id = ?1";
    let map_row = |row: &rusqlite::Row| -> rusqlite::Result<(String, HistoryTurn)> {
        let role: String = row.get(0)?;
        let input_mode: String = row.get(1)?;
        let content: String = row.get(2)?;
        let image_prompt: Option<String> = row.get(3)?;
        let content = match image_prompt {
            Some(prompt) => {
                format!("{content}\n\n[A scene image was generated here, depicting: {prompt}]")
            }
            None => content,
        };
        Ok((
            input_mode,
            HistoryTurn {
                is_player: role == "player",
                content,
            },
        ))
    };
    if let Some(seq) = before_seq {
        let mut stmt = conn.prepare(&format!(
            "{SELECT} AND passages.seq < ?2 ORDER BY passages.seq DESC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(rusqlite::params![branch_id, seq, HISTORY_LIMIT], map_row)?;
        for row in rows {
            rows_out.push(row?);
        }
    } else {
        let mut stmt = conn.prepare(&format!("{SELECT} ORDER BY passages.seq DESC LIMIT ?2"))?;
        let rows = stmt.query_map(rusqlite::params![branch_id, HISTORY_LIMIT], map_row)?;
        for row in rows {
            rows_out.push(row?);
        }
    }
    rows_out.reverse();
    Ok(drop_stale_story_drafts(rows_out))
}

fn drop_stale_story_drafts(rows: Vec<(String, HistoryTurn)>) -> Vec<HistoryTurn> {
    let newest = rows.len().saturating_sub(1);
    rows.into_iter()
        .enumerate()
        .filter(|(i, (input_mode, _))| input_mode != "story" || *i == newest)
        .map(|(_, (_, turn))| turn)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(content: &str) -> HistoryTurn {
        HistoryTurn {
            is_player: false,
            content: content.to_string(),
        }
    }

    #[test]
    fn drops_draft_once_its_completion_is_newest() {
        let rows = vec![
            ("story".to_string(), turn("terse draft")),
            ("generated_story".to_string(), turn("completed passage")),
        ];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "completed passage");
    }

    #[test]
    fn keeps_trailing_draft_for_completion() {
        let rows = vec![
            ("generated".to_string(), turn("scene")),
            ("story".to_string(), turn("draft")),
        ];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].content, "draft");
    }

    #[test]
    fn keeps_draft_when_window_excludes_completion() {
        let rows = vec![("story".to_string(), turn("draft"))];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "draft");
    }

    #[test]
    fn drops_stranded_older_draft() {
        let rows = vec![
            ("story".to_string(), turn("stranded draft")),
            ("story".to_string(), turn("newer draft")),
        ];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].content, "newer draft");
    }

    #[test]
    fn leaves_non_draft_history_untouched() {
        let rows = vec![
            ("do".to_string(), turn("player action")),
            ("generated".to_string(), turn("narration")),
            ("generated_continue".to_string(), turn("continued scene")),
        ];
        let out = drop_stale_story_drafts(rows);
        assert_eq!(out.len(), 3);
    }
}
