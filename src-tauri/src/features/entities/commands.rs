use std::collections::HashMap;
use tauri::State;

use crate::features::turn::TurnGate;
use crate::shared::db::{blocking, with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

use super::attributes::{
    list_entity_attributes_for_entities_sync, list_entity_attributes_sync,
    remove_entity_attribute_sync, set_entity_attribute_sync,
};
use super::model::{AttributeRegistryEntry, CharacterFields, CharacterPatch, Entity, EntityAttributeValue, CHARACTER, RELATIONSHIP};
use super::registry::list_attribute_registry_sync;
use super::repository::{create_character_sync, delete_entity_sync, load_entity_raw, update_character_sync, update_link_sync};
use super::{create_entity_sync, list_entities_sync, update_entity_sync};

#[tauri::command]
pub fn list_entities(
    pool: State<Pool>,
    story_id: String,
    kind: Option<String>,
) -> AppResult<Vec<Entity>> {
    let conn = pool.get()?;
    list_entities_sync(&conn, &story_id, kind.as_deref())
}

#[tauri::command]
pub async fn create_entity(
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    story_id: String,
    kind: String,
    name: String,
    appearance_anchor: Option<String>,
    fields: Option<CharacterFields>,
) -> AppResult<Entity> {
    let ticket = gate.check_idle(&story_id)?;
    let gate = gate.inner().clone();
    let pool = pool.inner().clone();
    blocking(move || {
        with_transaction(&pool, |tx| {
            gate.still_idle(&ticket)?;
            match fields {
                Some(fields) => {
                    if kind != CHARACTER {
                        return Err(AppError::Invalid("kind must be character".into()));
                    }
                    create_character_sync(tx, &story_id, &name, fields, "user", None, None)
                }
                None => create_entity_sync(
                    tx, &story_id, &kind, &name, appearance_anchor.as_deref(), "user", None,
                ),
            }
        })
    })
    .await
}

#[tauri::command]
pub async fn update_entity(
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    story_id: String,
    entity_id: String,
    name: Option<String>,
    appearance_anchor: Option<String>,
    fields: Option<CharacterPatch>,
) -> AppResult<Entity> {
    let ticket = gate.check_idle(&story_id)?;
    let gate = gate.inner().clone();
    let pool = pool.inner().clone();
    blocking(move || {
        with_transaction(&pool, |tx| {
            gate.still_idle(&ticket)?;
            let before = load_entity_raw(tx, &story_id, &entity_id)?
                .ok_or_else(|| AppError::NotFound(format!("entity {entity_id}")))?;
            if before.kind == RELATIONSHIP {
                return update_link_sync(tx, &story_id, &entity_id, name.as_deref(), None, None, "user", None, None);
            }
            match fields {
                Some(fields) => update_character_sync(
                    tx, &story_id, &entity_id, name.as_deref(), &fields, "user", None, None,
                ),
                None => update_entity_sync(
                    tx, &story_id, &entity_id, name.as_deref().unwrap_or(&before.name), appearance_anchor.as_deref(), "user", None, None,
                ),
            }
        })
    })
    .await
}

#[tauri::command]
pub async fn delete_entity(
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    story_id: String,
    entity_id: String,
) -> AppResult<()> {
    let ticket = gate.check_idle(&story_id)?;
    let gate = gate.inner().clone();
    let pool = pool.inner().clone();
    blocking(move || {
        with_transaction(&pool, |tx| {
            gate.still_idle(&ticket)?;
            delete_entity_sync(tx, &story_id, &entity_id)
        })
    })
    .await
}

#[tauri::command]
pub fn list_entity_attributes(
    pool: State<Pool>,
    story_id: String,
    entity_id: String,
) -> AppResult<Vec<EntityAttributeValue>> {
    let conn = pool.get()?;
    list_entity_attributes_sync(&conn, &story_id, &entity_id)
}

#[tauri::command]
pub fn list_story_attributes(
    pool: State<Pool>,
    story_id: String,
) -> AppResult<HashMap<String, Vec<EntityAttributeValue>>> {
    let conn = pool.get()?;
    let entities = list_entities_sync(&conn, &story_id, None)?;
    let ids = entities.iter().map(|entity| entity.id.as_str()).collect::<Vec<_>>();
    list_entity_attributes_for_entities_sync(&conn, &story_id, &ids)
}

#[tauri::command]
pub fn list_attribute_registry(pool: State<Pool>) -> AppResult<Vec<AttributeRegistryEntry>> {
    let conn = pool.get()?;
    list_attribute_registry_sync(&conn)
}

#[tauri::command]
pub async fn set_entity_attribute(
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    story_id: String,
    entity_id: String,
    attribute_id: String,
    value: f64,
) -> AppResult<EntityAttributeValue> {
    let ticket = gate.check_idle(&story_id)?;
    let gate = gate.inner().clone();
    let pool = pool.inner().clone();
    blocking(move || {
        with_transaction(&pool, |tx| {
            gate.still_idle(&ticket)?;
            set_entity_attribute_sync(tx, &story_id, &entity_id, &attribute_id, value)
        })
    })
    .await
}

#[tauri::command]
pub async fn remove_entity_attribute(
    pool: State<'_, Pool>,
    gate: State<'_, TurnGate>,
    story_id: String,
    entity_id: String,
    attribute_id: String,
) -> AppResult<()> {
    let ticket = gate.check_idle(&story_id)?;
    let gate = gate.inner().clone();
    let pool = pool.inner().clone();
    blocking(move || {
        with_transaction(&pool, |tx| {
            gate.still_idle(&ticket)?;
            remove_entity_attribute_sync(tx, &story_id, &entity_id, &attribute_id)
        })
    })
    .await
}
