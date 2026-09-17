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
            features::timeline::list_timeline_entries,
            features::narration::submit_story,
            features::narration::submit_turn,
            features::narration::submit_guide,
            features::narration::continue_scene,
            features::narration::retry_narration,
            features::narration::generate_narration_variant,
            features::narration::list_narration_variants,
            features::narration::select_narration_variant,
            features::narration::edit_timeline_entry,
            features::narration::erase_last_exchange,
            features::settings::get_text_model_settings,
            features::settings::save_text_model_settings,
            features::settings::get_image_model_settings,
            features::settings::save_image_model_settings,
            features::settings::get_narrator_memory_settings,
            features::settings::save_narrator_memory_settings,
            features::images::generate_scene_image,
            features::images::list_images_for_story,
            features::entities::list_entities,
            features::entities::create_entity,
            features::entities::update_entity,
            features::entities::delete_entity,
            features::entities::attributes::list_entity_attributes,
            features::entities::attributes::list_attribute_registry,
            features::entities::attributes::set_entity_attribute,
            features::entities::attributes::remove_entity_attribute,
            features::dicerolls::commands::get_story_diceroll_settings,
            features::dicerolls::commands::save_story_diceroll_settings,
            features::dicerolls::commands::list_rolls_for_story,
            features::dicerolls::commands::list_roll_details_for_entry,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
