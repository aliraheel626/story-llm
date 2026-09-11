mod commands;
mod db;
mod error;
mod images;
mod mechanics;
mod models;
mod narrator;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build());

    #[cfg(debug_assertions)]
    {
        builder = builder.plugin(tauri_plugin_pilot::init());
    }

    builder
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let pool = db::init_pool(&app_data_dir)?;
            app.manage(pool);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::stories::list_stories,
            commands::stories::create_story,
            commands::stories::rename_story,
            commands::stories::get_author_note,
            commands::stories::save_author_note,
            commands::passages::list_passages,
            commands::passages::submit_story,
            commands::passages::submit_turn,
            commands::passages::submit_guide,
            commands::passages::continue_scene,
            commands::passages::retry_passage,
            commands::passages::swipe_passage,
            commands::passages::list_variants,
            commands::passages::switch_variant,
            commands::passages::edit_passage,
            commands::passages::erase_last_exchange,
            commands::settings::get_text_model_settings,
            commands::settings::save_text_model_settings,
            commands::settings::get_image_model_settings,
            commands::settings::save_image_model_settings,
            commands::images::generate_scene_image,
            commands::images::list_images_for_passage,
            commands::images::list_images_for_branch,
            commands::entities::list_entities,
            commands::entities::create_entity,
            commands::entities::update_entity,
            commands::entities::delete_entity,
            commands::mechanics::get_story_mechanics_settings,
            commands::mechanics::save_story_mechanics_settings,
            commands::mechanics::list_entity_attributes,
            commands::mechanics::list_attribute_registry,
            commands::mechanics::list_rolls_for_passage,
            commands::mechanics::list_rolls_for_branch,
            commands::mechanics::get_roll_detail,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
