mod ai;
mod features;
mod shared;

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
            let pool = shared::db::init_pool(&app_data_dir)?;
            app.manage(pool);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            features::stories::list_stories,
            features::stories::create_story,
            features::stories::rename_story,
            features::stories::delete_story,
            features::stories::get_author_note,
            features::stories::save_author_note,
            features::passages::list_passages,
            features::passages::submit_story,
            features::passages::submit_turn,
            features::passages::submit_guide,
            features::passages::continue_scene,
            features::passages::retry_passage,
            features::passages::swipe_passage,
            features::passages::list_variants,
            features::passages::switch_variant,
            features::passages::edit_passage,
            features::passages::erase_last_exchange,
            features::settings::get_text_model_settings,
            features::settings::save_text_model_settings,
            features::settings::get_image_model_settings,
            features::settings::save_image_model_settings,
            features::images::generate_scene_image,
            features::images::list_images_for_passage,
            features::images::list_images_for_branch,
            features::entities::list_entities,
            features::entities::create_entity,
            features::entities::update_entity,
            features::entities::delete_entity,
            features::mechanics::commands::get_story_mechanics_settings,
            features::mechanics::commands::save_story_mechanics_settings,
            features::mechanics::commands::list_entity_attributes,
            features::mechanics::commands::list_attribute_registry,
            features::mechanics::commands::list_rolls_for_passage,
            features::mechanics::commands::list_rolls_for_branch,
            features::mechanics::commands::get_roll_detail,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
