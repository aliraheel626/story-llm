use tauri::State;

use crate::features::ledger::turn_tx::TurnGate;
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::AppResult;

use super::attributes::{
    list_entity_attributes_sync, remove_entity_attribute_sync, set_entity_attribute_sync,
};
use super::model::{AttributeRegistryEntry, Entity, EntityAttributeValue};
use super::registry::list_attribute_registry_sync;
use super::repository::delete_entity_sync;
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
) -> AppResult<Entity> {
    let ticket = gate.check_idle(&story_id)?;
    let gate = gate.inner().clone();
    let pool = pool.inner().clone();
    crate::shared::db::blocking(move || {
        with_transaction(&pool, |tx| {
            gate.still_idle(&ticket)?;
            create_entity_sync(
                tx,
                &story_id,
                &kind,
                &name,
                appearance_anchor.as_deref(),
                "user",
                None,
            )
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
    name: String,
    appearance_anchor: Option<String>,
) -> AppResult<Entity> {
    let ticket = gate.check_idle(&story_id)?;
    let gate = gate.inner().clone();
    let pool = pool.inner().clone();
    crate::shared::db::blocking(move || {
        with_transaction(&pool, |tx| {
            gate.still_idle(&ticket)?;
            update_entity_sync(
                tx,
                &story_id,
                &entity_id,
                &name,
                appearance_anchor.as_deref(),
                "user",
                None,
                None,
            )
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
    crate::shared::db::blocking(move || {
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
    crate::shared::db::blocking(move || {
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
    crate::shared::db::blocking(move || {
        with_transaction(&pool, |tx| {
            gate.still_idle(&ticket)?;
            remove_entity_attribute_sync(tx, &story_id, &entity_id, &attribute_id)
        })
    })
    .await
}
