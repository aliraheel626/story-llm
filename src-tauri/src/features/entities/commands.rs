use tauri::State;

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
pub fn create_entity(
    pool: State<Pool>,
    story_id: String,
    kind: String,
    name: String,
    appearance_anchor: Option<String>,
) -> AppResult<Entity> {
    with_transaction(pool.inner(), |tx| {
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
}

#[tauri::command]
pub fn update_entity(
    pool: State<Pool>,
    story_id: String,
    entity_id: String,
    name: String,
    appearance_anchor: Option<String>,
) -> AppResult<Entity> {
    with_transaction(pool.inner(), |tx| {
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
}

#[tauri::command]
pub fn delete_entity(pool: State<Pool>, story_id: String, entity_id: String) -> AppResult<()> {
    with_transaction(pool.inner(), |tx| {
        delete_entity_sync(tx, &story_id, &entity_id)
    })
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
pub fn set_entity_attribute(
    pool: State<Pool>,
    story_id: String,
    entity_id: String,
    attribute_id: String,
    value: f64,
) -> AppResult<EntityAttributeValue> {
    with_transaction(pool.inner(), |tx| {
        set_entity_attribute_sync(tx, &story_id, &entity_id, &attribute_id, value)
    })
}

#[tauri::command]
pub fn remove_entity_attribute(
    pool: State<Pool>,
    story_id: String,
    entity_id: String,
    attribute_id: String,
) -> AppResult<()> {
    with_transaction(pool.inner(), |tx| {
        remove_entity_attribute_sync(tx, &story_id, &entity_id, &attribute_id)
    })
}
