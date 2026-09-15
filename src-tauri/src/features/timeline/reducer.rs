use std::collections::HashMap;

use super::model::{kind, NarrationVariant, TimelineEntry};

/// For a given target entry, which underlying entry (the base narration, or
/// one of its variants) is currently selected to supply its content. Folding
/// this separately from edits is what lets an edit "stick" to the variant it
/// was made against instead of being wiped out by an unrelated re-selection.
pub fn active_variant_id(entries: &[TimelineEntry], target: &str) -> String {
    entries
        .iter()
        .rev()
        .find(|e| {
            e.kind == kind::NARRATION_SELECTED && e.target_entry_id.as_deref() == Some(target)
        })
        .and_then(|e| e.payload.get("selected_entry_id"))
        .and_then(|v| v.as_str())
        .unwrap_or(target)
        .to_string()
}

/// Latest edited text per (target entry, the variant it was edited against).
/// `applies_to` defaults to the target itself for edits made before this
/// field existed, preserving old data.
fn edits_by_target_and_variant(entries: &[TimelineEntry]) -> HashMap<(String, String), String> {
    let mut edits = HashMap::new();
    for entry in entries {
        if entry.kind != kind::CONTENT_EDITED {
            continue;
        }
        if let (Some(target), Some(content)) = (&entry.target_entry_id, &entry.content) {
            let applies_to = entry
                .payload
                .get("applies_to")
                .and_then(|v| v.as_str())
                .unwrap_or(target.as_str())
                .to_string();
            edits.insert((target.clone(), applies_to), content.clone());
        }
    }
    edits
}

pub fn active_visible_entries(entries: &[TimelineEntry]) -> Vec<TimelineEntry> {
    let by_id: HashMap<&str, &TimelineEntry> = entries.iter().map(|e| (e.id.as_str(), e)).collect();
    let edits = edits_by_target_and_variant(entries);

    entries
        .iter()
        .filter(|e| e.kind == kind::PLAYER_MESSAGE || e.kind == kind::NARRATION)
        .map(|e| {
            let mut active = e.clone();
            let variant_id = active_variant_id(entries, &e.id);
            if let Some(edited) = edits.get(&(e.id.clone(), variant_id.clone())) {
                active.content = Some(edited.clone());
            } else if variant_id != e.id {
                if let Some(content) = by_id
                    .get(variant_id.as_str())
                    .and_then(|v| v.content.as_ref())
                {
                    active.content = Some(content.clone());
                }
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
    let edits = edits_by_target_and_variant(entries);

    let mut out = Vec::new();
    if let Some(base) = entries.iter().find(|e| e.id == entry_id) {
        if let Some(content) = &base.content {
            let content = edits
                .get(&(entry_id.to_string(), base.id.clone()))
                .cloned()
                .unwrap_or_else(|| content.clone());
            out.push(NarrationVariant {
                id: base.id.clone(),
                entry_id: entry_id.to_string(),
                content,
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
                let content = edits
                    .get(&(entry_id.to_string(), e.id.clone()))
                    .cloned()
                    .or_else(|| e.content.clone())?;
                Some(NarrationVariant {
                    id: e.id.clone(),
                    entry_id: entry_id.to_string(),
                    content,
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
                json!({"applies_to":"v"}),
            ),
        ];
        let visible = active_visible_entries(&rows);
        assert_eq!(visible[0].content.as_deref(), Some("edited"));
        assert_eq!(rows[0].content.as_deref(), Some("original"));
    }

    #[test]
    fn editing_the_active_variant_survives_reselecting_it_later() {
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
                "s1",
                kind::NARRATION_SELECTED,
                None,
                Some("n"),
                json!({"selected_entry_id":"v"}),
            ),
            entry(
                "e",
                kind::CONTENT_EDITED,
                Some("edited variant"),
                Some("n"),
                json!({"applies_to":"v"}),
            ),
            // Re-selecting the same variant later must not discard the edit
            // made against it.
            entry(
                "s2",
                kind::NARRATION_SELECTED,
                None,
                Some("n"),
                json!({"selected_entry_id":"v"}),
            ),
        ];
        let visible = active_visible_entries(&rows);
        assert_eq!(visible[0].content.as_deref(), Some("edited variant"));

        let variants = variants_for_entry(&rows, "n");
        let v = variants.iter().find(|v| v.id == "v").unwrap();
        assert_eq!(v.content, "edited variant");
    }

    #[test]
    fn editing_the_active_variant_does_not_leak_into_a_different_variant() {
        let rows = vec![
            entry("n", kind::NARRATION, Some("original"), None, json!({})),
            entry(
                "v1",
                kind::NARRATION_VARIANT,
                Some("first"),
                Some("n"),
                json!({}),
            ),
            entry(
                "v2",
                kind::NARRATION_VARIANT,
                Some("second"),
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
                "e",
                kind::CONTENT_EDITED,
                Some("edited first"),
                Some("n"),
                json!({"applies_to":"v1"}),
            ),
            entry(
                "s2",
                kind::NARRATION_SELECTED,
                None,
                Some("n"),
                json!({"selected_entry_id":"v2"}),
            ),
        ];
        // Switching to the un-edited variant must show its own text, not the
        // other variant's edit.
        assert_eq!(
            active_visible_entries(&rows)[0].content.as_deref(),
            Some("second")
        );
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
