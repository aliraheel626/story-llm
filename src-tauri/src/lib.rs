mod ai;
mod features;
mod prompts;
mod shared;

use tauri::{Emitter, Manager};
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
        .register_asynchronous_uri_scheme_protocol("storyimg", |ctx, request, responder| {
            let id = request.uri().path().rsplit('/').next().unwrap_or_default().to_string();
            let pool = ctx.app_handle().state::<shared::db::Pool>().inner().clone();
            tauri::async_runtime::spawn_blocking(move || {
                let image = pool.get().ok().and_then(|conn| {
                    features::ledger::attachments::image_blob(&conn, &id)
                        .ok()
                        .flatten()
                });
                let response = match image {
                    Some((media_type, bytes)) => http::Response::builder()
                        .header(http::header::CONTENT_TYPE, media_type)
                        .body(bytes)
                        .expect("valid image response"),
                    None => http::Response::builder()
                        .status(http::StatusCode::NOT_FOUND)
                        .body(Vec::new())
                        .expect("valid not-found response"),
                };
                responder.respond(response);
            });
        })
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let pool = shared::db::init_pool(&app_data_dir)?;
            let refresh_pool = pool.clone();
            app.manage(pool);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if features::settings::refresh_missing_capabilities(refresh_pool).await {
                    if let Err(error) = handle.emit("text-model-capabilities-refreshed", ()) {
                        log::warn!("could not notify text model capability refresh: {error}");
                    }
                }
            });
            app.manage(features::turn::TurnGate::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            features::stories::list_stories,
            features::stories::new_story,
            features::stories::rename_story,
            features::stories::delete_story,
            features::context::get_story_context_settings,
            features::context::save_story_context_settings,
            features::context::get_story_injection_settings,
            features::context::save_story_injection_settings,
            features::narrator::preview_story_context,
            features::ledger::list_ledger_entries,
            features::turn::submit_turn,
            features::turn::retry_narration,
            features::ledger::edit_ledger_entry,
            features::ledger::erase_last_exchange,
            features::stories::get_story_narrator_tools,
            features::stories::save_story_narrator_tools,
            features::stories::get_story_reasoning_effort,
            features::stories::save_story_reasoning_effort,
            features::settings::get_text_model_settings,
            features::settings::save_text_model_settings,
            features::settings::get_image_model_settings,
            features::settings::save_image_model_settings,
            features::images::list_images_for_story,
            features::stats::get_story_stats,
            features::entities::list_entities,
            features::entities::create_entity,
            features::entities::update_entity,
            features::entities::delete_entity,
            features::entities::list_entity_attributes,
            features::entities::list_attribute_registry,
            features::entities::set_entity_attribute,
            features::entities::remove_entity_attribute,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
