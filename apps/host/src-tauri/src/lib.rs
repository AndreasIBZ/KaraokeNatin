#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod room_state;
mod commands;
mod metadata;
mod network;
mod playlist_importer;
mod web_server;
mod youtube;
pub mod peer_server;
mod signaling;

use room_state::{RoomStateManager, PlaylistStore, SessionHistoryStore};
use uuid::Uuid;
use tauri::{Manager, WindowEvent};
use std::path::Path;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

static SESSION_HISTORY_EXPORTED: AtomicBool = AtomicBool::new(false);

fn copy_file_if_missing(from: &Path, to: &Path) {
    if !from.exists() || to.exists() {
        return;
    }
    if let Some(parent) = to.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match fs::copy(from, to) {
        Ok(_) => log::info!("Migrated data file from {:?} to {:?}", from, to),
        Err(e) => log::error!("Failed to migrate data file from {:?} to {:?}: {}", from, to, e),
    }
}

fn migrate_legacy_tauri_data_dir(app: &tauri::App, target_dir: &Path) {
    let Ok(legacy_dir) = app.path().app_local_data_dir() else {
        return;
    };
    if legacy_dir == target_dir || !legacy_dir.exists() {
        return;
    }

    copy_file_if_missing(
        &legacy_dir.join("playlists.json"),
        &target_dir.join("playlists.json"),
    );

    let legacy_sessions = legacy_dir.join("session-history");
    if let Ok(entries) = fs::read_dir(&legacy_sessions) {
        for entry in entries.flatten() {
            let from = entry.path();
            if !from.is_file() {
                continue;
            }
            copy_file_if_missing(
                &from,
                &target_dir.join("session-history").join(entry.file_name()),
            );
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // PlaylistStore is always available (both Host & Guest modes)
    let playlist_store = PlaylistStore::new();
    let session_history_store = SessionHistoryStore::new();

    // Room state manager — playlists injected from store
    let initial_room_id = "pending".to_string();
    let initial_peer_id = Uuid::new_v4().to_string();
    let room_manager = RoomStateManager::new(
        initial_room_id,
        initial_peer_id,
        playlist_store.get_all(),
    );

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Backs `commands::save_collection_to_file` / `load_collection_from_file`:
        // its `FsExt::fs()` can read/write through a `FilePath` returned by the
        // dialog plugin even when that's an Android `content://` URI rather than
        // a real filesystem path.
        .plugin(tauri_plugin_fs::init());

    #[cfg(not(target_os = "android"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = app.get_webview_window("main").expect("no main window").set_focus();
        }));
    }

    builder
        .manage(playlist_store)
        .manage(session_history_store)
        .manage(room_manager)
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Initialize PlaylistStore with persistent path
            let app_handle = app.handle();
            let playlist_store = app_handle.state::<PlaylistStore>();
            let session_history_store = app_handle.state::<SessionHistoryStore>();
            let room_manager = app_handle.state::<RoomStateManager>();
            
            match commands::festejar_data_dir(app_handle) {
                Ok(path) => {
                    log::info!("Resolved FESTEJAR local data dir: {:?}", path);
                    migrate_legacy_tauri_data_dir(app, &path);
                    let loaded_playlists = playlist_store.initialize(path.clone());
                    session_history_store.initialize(path);
                    
                    // Sync initial playlists to RoomStateManager
                    let mut state = room_manager.write();
                    state.sync_playlists(loaded_playlists);
                }
                Err(e) => log::error!("Failed to resolve app local data dir: {}", e),
            }

            // NOTE: Web server is now started lazily via start_host_server command
            // when the user picks Host Mode from the landing screen.

            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed) {
                if !SESSION_HISTORY_EXPORTED.swap(true, Ordering::SeqCst) {
                    let session_history = window.app_handle().state::<SessionHistoryStore>();
                    match session_history.export_current_session() {
                        Ok(Some(path)) => log::info!("Exported session history to {:?}", path),
                        Ok(None) => {}
                        Err(e) => log::error!("Failed to export session history: {}", e),
                    }
                }
                if let Some(player_display) = window.app_handle().get_webview_window("player-display") {
                    let _ = player_display.destroy();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            // Host-mode room commands
            commands::create_room,
            commands::get_qr_url,
            commands::get_server_port,
            commands::get_room_state,
            commands::search_youtube,
            commands::queue_search_result,
            commands::process_command,
            commands::update_player_state,
            commands::set_auto_play_next,
            commands::get_session_history,
            commands::export_session_history,
            commands::open_player_display,
            commands::close_player_display,
            commands::shutdown_host_session,
            commands::set_player_display_fullscreen,
            commands::export_collection,
            commands::start_host_server,
            // Standalone playlist commands (available in all modes)
            commands::get_playlists,
            commands::playlist_create_collection,
            commands::playlist_delete_collection,
            commands::playlist_rename_collection,
            commands::playlist_set_visibility,
            commands::playlist_add_song,
            commands::playlist_remove_song,
            commands::playlist_resolve_song,
            commands::playlist_queue_collection,
            commands::playlist_move_songs,
            commands::playlist_import_collection,
            commands::preview_spotify_playlist_import,
            commands::preview_karaoke_json_playlist_import,
            commands::preview_text_playlist_import,
            commands::confirm_playlist_import,
            commands::save_collection_to_file,
            commands::load_collection_from_file,
            // Diagnostics
            commands::get_app_settings_info,
            commands::open_data_folder,
            commands::open_session_history_folder,
            commands::open_log_folder,
            commands::open_github_repository,
            commands::report_issue,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
