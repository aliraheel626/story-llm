use std::collections::HashMap;

use super::model::{kind, NarrationVariant, TimelineEntry};

pub fn active_visible_entries(entries: &[TimelineEntry]) -> Vec<TimelineEntry> {
    let by_id: HashMap<&str, &TimelineEntry> = entries.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut active_content: HashMap<String, String> = HashMap::new();

    for entry in entries {
        match entry.kind.as_str() {
            kind::CONTENT_EDITED => {
                if let (Some(target), Some(content)) = (&entry.target_entry_id, &entry.content) {
                    active_content.insert(target.clone(), content.clone());
                }
            }
            kind::NARRATION_SELECTED => {
                let Some(target) = &entry.target_entry_id else {
                    continue;
                };
                let Some(selected_id) = entry
                    .payload
                    .get("selected_entry_id")
                    .and_then(|v| v.as_str())
                else {
                    continue;
                };
                if let Some(selected) = by_id.get(selected_id).and_then(|e| e.content.as_ref()) {
                    active_content.insert(target.clone(), selected.clone());
                }
            }
            _ => {}
        }
    }

    entries
        .iter()
        .filter(|e| e.kind == kind::PLAYER_MESSAGE || e.kind == kind::NARRATION)
        .map(|e| {
            let mut active = e.clone();
            if let Some(content) = active_content.get(&e.id) {
                active.content = Some(content.clone());
            }
            active
        })
        .collect()
}

pub fn variants_for_entry(entries: &[TimelineEntry], entry_id: &str) -> Vec<NarrationVariant> {
    let selected = entries
        .iter()
        .rev()
        .find(|e| {
            e.kind == kind::NARRATION_SELECTED && e.target_entry_id.as_deref() == Some(entry_id)
        })
        .and_then(|e| e.payload.get("selected_entry_id"))
        .and_then(|v| v.as_str());

    let mut out = Vec::new();
    if let Some(base) = entries.iter().find(|e| e.id == entry_id) {
        if let Some(content) = &base.content {
            out.push(NarrationVariant {
                id: base.id.clone(),
                entry_id: entry_id.to_string(),
                content: content.clone(),
                is_selected: selected.is_none() || selected == Some(base.id.as_str()),
                created_at: base.created_at.clone(),
            });
        }
    }
    out.extend(
        entries
            .iter()
            .filter(|e| {
                e.kind == kind::NARRATION_VARIANT && e.target_entry_id.as_deref() == Some(entry_id)
            })
            .filter_map(|e| {
                Some(NarrationVariant {
                    id: e.id.clone(),
                    entry_id: entry_id.to_string(),
                    content: e.content.clone()?,
                    is_selected: selected == Some(e.id.as_str()),
                    created_at: e.created_at.clone(),
                })
            }),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(
        id: &str,
        kind: &str,
        content: Option<&str>,
        target: Option<&str>,
        payload: serde_json::Value,
    ) -> TimelineEntry {
        TimelineEntry {
            id: id.into(),
            branch_id: "b".into(),
            seq: 0,
            kind: kind.into(),
            visibility: "hidden".into(),
            content: content.map(str::to_string),
            payload,
            target_entry_id: target.map(str::to_string),
            created_at: "now".into(),
        }
    }

    #[test]
    fn selection_and_edit_fold_without_mutating_original() {
        let rows = vec![
            entry("n", kind::NARRATION, Some("original"), None, json!({})),
            entry(
                "v",
                kind::NARRATION_VARIANT,
                Some("variant"),
                Some("n"),
                json!({}),
            ),
            entry(
                "s",
                kind::NARRATION_SELECTED,
                None,
                Some("n"),
                json!({"selected_entry_id":"v"}),
            ),
            entry(
                "e",
                kind::CONTENT_EDITED,
                Some("edited"),
                Some("n"),
                json!({}),
            ),
        ];
        let visible = active_visible_entries(&rows);
        assert_eq!(visible[0].content.as_deref(), Some("edited"));
        assert_eq!(rows[0].content.as_deref(), Some("original"));
    }

    #[test]
    fn latest_variant_selection_is_active_and_all_variants_remain_available() {
        let rows = vec![
            entry("n", kind::NARRATION, Some("original"), None, json!({})),
            entry(
                "v1",
                kind::NARRATION_VARIANT,
                Some("first variant"),
                Some("n"),
                json!({}),
            ),
            entry(
                "v2",
                kind::NARRATION_VARIANT,
                Some("second variant"),
                Some("n"),
                json!({}),
            ),
            entry(
                "s1",
                kind::NARRATION_SELECTED,
                None,
                Some("n"),
                json!({"selected_entry_id":"v1"}),
            ),
            entry(
                "s2",
                kind::NARRATION_SELECTED,
                None,
                Some("n"),
                json!({"selected_entry_id":"v2"}),
            ),
        ];

        assert_eq!(
            active_visible_entries(&rows)[0].content.as_deref(),
            Some("second variant")
        );
        let variants = variants_for_entry(&rows, "n");
        assert_eq!(variants.len(), 3);
        assert_eq!(
            variants
                .iter()
                .find(|variant| variant.is_selected)
                .unwrap()
                .id,
            "v2"
        );
    }
}
