mod ai;
mod features;
mod prompts;
mod shared;

use tauri::Manager;
use tauri_plugin_log::{Target, TargetKind};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir { file_name: None }),
                ])
                .build(),
        );

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
            features::narration::author_note::get_author_note,
            features::narration::author_note::save_author_note,
            features::narration::author_note::get_author_note_enabled,
            features::narration::author_note::set_author_note_enabled,
            features::ledger::list_ledger_entries,
            features::narration::submit_turn,
            features::narration::retry_narration,
            features::narration::generate_narration_variant,
            features::narration::list_narration_variants,
            features::narration::select_narration_variant,
            features::narration::edit_ledger_entry,
            features::narration::erase_last_exchange,
            features::settings::get_text_model_settings,
            features::settings::save_text_model_settings,
            features::settings::get_image_model_settings,
            features::settings::save_image_model_settings,
            features::settings::get_context_injection_settings,
            features::settings::save_context_injection_settings,
            features::settings::get_ledger_retention_settings,
            features::settings::save_ledger_retention_settings,
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
