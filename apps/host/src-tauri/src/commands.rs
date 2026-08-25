use crate::playlist_importer::{
    imported_playlist_to_collection, preview_from_imported, spotify_playlist_id_from_import,
    ImportPreview, ImportedPlaylist, KaraokeJsonImporter, PlaylistImporter, SpotifyHttpClient,
    SpotifyPlaylistImporter, TextPlaylistImporter,
};
use crate::room_state::{
    CollectionVisibility, PlayerStatus, PlaylistCollection, PlaylistStore, ResolutionStatus,
    RoomStateManager, SessionHistoryStore, Song, SongSource,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;

/// Guard so start_host_server is idempotent
static SERVER_STARTED: AtomicBool = AtomicBool::new(false);
const PLAYER_DISPLAY_WINDOW_LABEL: &str = "player-display";

/// Client command types (from P2P protocol)
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
#[allow(non_camel_case_types)]
pub enum ClientCommand {
    PLAY,
    PAUSE,
    SKIP {
        #[serde(default, rename = "autoPlay")]
        auto_play: Option<bool>,
    },
    SEEK { time: f64 },
    SET_VOLUME { volume: u8 },
    TOGGLE_MUTE,
    ADD_SONG {
        #[serde(rename = "youtubeUrl")]
        youtube_url: String,
        #[serde(rename = "addedBy")]
        added_by: Option<String>,
    },
    REMOVE_SONG {
        #[serde(rename = "songId")]
        song_id: String,
    },
    MOVE_SONG_UP {
        #[serde(rename = "songId")]
        song_id: String,
    },
    MOVE_SONG_DOWN {
        #[serde(rename = "songId")]
        song_id: String,
    },
    MOVE_SONG_TO_TOP {
        #[serde(rename = "songId")]
        song_id: String,
    },
    MOVE_SONG_TO_BOTTOM {
        #[serde(rename = "songId")]
        song_id: String,
    },
    REORDER_QUEUE {
        #[serde(rename = "songId")]
        song_id: String,
        #[serde(rename = "newIndex")]
        new_index: usize,
    },
    SET_DISPLAY_NAME { name: String },
    PING,
    // Collection-based playlist commands
    PLAYLIST_ADD {
        #[serde(rename = "youtubeUrl")]
        youtube_url: String,
        #[serde(rename = "collectionId")]
        collection_id: String,
        #[serde(rename = "addedBy")]
        added_by: Option<String>,
    },
    PLAYLIST_REMOVE {
        #[serde(rename = "songId")]
        song_id: String,
        #[serde(rename = "collectionId")]
        collection_id: String,
    },
    PLAYLIST_TO_QUEUE {
        #[serde(rename = "songId")]
        song_id: String,
        #[serde(rename = "collectionId")]
        collection_id: String,
    },
    // Collection management commands
    CREATE_COLLECTION {
        name: String,
        #[serde(default = "default_public_visibility")]
        visibility: CollectionVisibility,
    },
    DELETE_COLLECTION {
        #[serde(rename = "collectionId")]
        collection_id: String,
    },
    RENAME_COLLECTION {
        #[serde(rename = "collectionId")]
        collection_id: String,
        name: String,
    },
    SET_COLLECTION_VISIBILITY {
        #[serde(rename = "collectionId")]
        collection_id: String,
        visibility: CollectionVisibility,
    },
    IMPORT_COLLECTION {
        data: String,
    },
}

fn default_public_visibility() -> CollectionVisibility {
    CollectionVisibility::Public
}

/// Create a new room
#[tauri::command]
pub fn create_room(
    state: tauri::State<RoomStateManager>,
    playlists: tauri::State<PlaylistStore>,
) -> Result<CreateRoomResponse, String> {
    // Generate unique room ID and join token
    let room_id = generate_room_id();
    let join_token = generate_join_token();
    
    // Start from a clean player/queue state, while keeping current playlists.
    state.write().reset_for_new_session(
        room_id.clone(),
        Uuid::new_v4().to_string(),
        playlists.get_all(),
    );
    
    log::info!("Created room: {} with token", room_id);
    
    Ok(CreateRoomResponse {
        room_id,
        join_token,
    })
}

/// Get the QR code URL for clients to connect
#[tauri::command]
pub fn get_qr_url() -> Result<String, String> {
    crate::network::generate_qr_url()
}

/// Get the web server port
#[tauri::command]
pub fn get_server_port() -> u16 {
    crate::web_server::get_server_port()
}

/// Get the current room state
#[tauri::command]
pub fn get_room_state(state: tauri::State<RoomStateManager>) -> Result<crate::room_state::RoomState, String> {
    Ok(state.clone_state())
}

/// Search YouTube for videos
#[tauri::command]
pub async fn search_youtube(
    query: String,
    limit: Option<u32>,
    karaoke_only: Option<bool>,
) -> Result<Vec<crate::youtube::SearchResult>, String> {
    let search_limit = limit.unwrap_or(10);
    crate::youtube::search_youtube(&query, search_limit, karaoke_only.unwrap_or(true)).await
}

/// Queue a result that already came from `search_youtube`.
///
/// The host UI has fresh title/channel/duration/thumbnail data in hand when the
/// user clicks "+ Queue". Re-fetching metadata at that point made the button
/// feel dead whenever YouTube metadata lookup was slow or flaky, even though
/// search itself had worked. This path still performs the optional embeddable
/// preflight, then queues immediately from the search payload.
#[tauri::command]
pub async fn queue_search_result(
    result: crate::youtube::SearchResult,
    added_by: Option<String>,
    state: tauri::State<'_, RoomStateManager>,
    history: tauri::State<'_, SessionHistoryStore>,
    app: AppHandle,
) -> Result<(), String> {
    crate::youtube::ensure_video_is_embeddable(&result.id).await?;

    let song = Song {
        id: Uuid::new_v4().to_string(),
        youtube_id: result.id.clone(),
        title: result.title,
        artist: result.channel,
        duration: parse_duration_label(&result.duration),
        original_title: None,
        original_artist: None,
        original_duration: None,
        thumbnail_url: result.thumbnail,
        added_by: added_by.unwrap_or_else(|| "Host".to_string()),
        added_at: chrono::Utc::now().timestamp_millis(),
        resolution_status: Some(ResolutionStatus::Resolved),
        source: Some(SongSource::Youtube {
            video_id: Some(result.id),
            url: None,
        }),
    };

    {
        let mut room_state = state.write();
        queue_song_if_embeddable(&mut room_state, song.clone(), true)?;
    }
    history.record_song(&song);

    emit_state(&app, &state)?;
    Ok(())
}

/// Process a client command
#[tauri::command]
pub async fn process_command(
    command: ClientCommand,
    state: tauri::State<'_, RoomStateManager>,
    playlists: tauri::State<'_, PlaylistStore>,
    history: tauri::State<'_, SessionHistoryStore>,
    app: AppHandle,
) -> Result<(), String> {
    log::info!("Processing command: {:?}", command);
    
    match command {
        ClientCommand::PLAY => {
            state.write().play();
        }
        ClientCommand::PAUSE => {
            state.write().pause();
        }
        ClientCommand::SKIP { auto_play } => {
            let should_auto_play = auto_play.unwrap_or_else(|| state.clone_player().auto_play_next);
            state.write().skip_song(should_auto_play);
        }
        ClientCommand::SEEK { time } => {
            state.write().seek(time);
        }
        ClientCommand::SET_VOLUME { volume } => {
            state.write().set_volume(volume);
        }
        ClientCommand::TOGGLE_MUTE => {
            state.write().toggle_mute();
        }
        ClientCommand::ADD_SONG { youtube_url, added_by } => {
            let youtube_id = extract_youtube_id(&youtube_url)
                .ok_or_else(|| "Invalid YouTube URL".to_string())?;
            crate::youtube::ensure_video_is_embeddable(&youtube_id).await?;
            
            match crate::metadata::fetch_metadata(&youtube_id).await {
                Ok(metadata) => {
                    let song = Song {
                        id: Uuid::new_v4().to_string(),
                        youtube_id: youtube_id.clone(),
                        title: metadata.title,
                        artist: metadata.artist,
                        duration: metadata.duration,
                        original_title: None,
                        original_artist: None,
                        original_duration: None,
                        thumbnail_url: metadata.thumbnail_url,
                        added_by: added_by.unwrap_or_else(|| "Guest".to_string()),
                        added_at: chrono::Utc::now().timestamp_millis(),
                        resolution_status: Some(ResolutionStatus::Resolved),
                        source: Some(SongSource::Youtube {
                            video_id: Some(youtube_id.clone()),
                            url: Some(youtube_url),
                        }),
                    };
                    let mut room_state = state.write();
                    queue_song_if_embeddable(&mut room_state, song.clone(), true)?;
                    history.record_song(&song);
                }
                Err(e) => {
                    log::error!("Failed to fetch metadata: {}", e);
                    return Err(format!("Failed to fetch song metadata: {}", e));
                }
            }
        }
        ClientCommand::REMOVE_SONG { song_id } => {
            if !state.write().remove_song(&song_id) {
                return Err("Song not found".to_string());
            }
        }
        ClientCommand::MOVE_SONG_UP { song_id } => {
            state.write().move_song_up(&song_id);
        }
        ClientCommand::MOVE_SONG_DOWN { song_id } => {
            state.write().move_song_down(&song_id);
        }
        ClientCommand::MOVE_SONG_TO_TOP { song_id } => {
            state.write().move_song_to_top(&song_id);
        }
        ClientCommand::MOVE_SONG_TO_BOTTOM { song_id } => {
            state.write().move_song_to_bottom(&song_id);
        }
        ClientCommand::REORDER_QUEUE { song_id, new_index } => {
            if !state.write().reorder_queue(&song_id, new_index) {
                return Err("Failed to reorder queue".to_string());
            }
        }
        ClientCommand::SET_DISPLAY_NAME { name } => {
            log::info!("Client set display name: {}", name);
        }
        ClientCommand::PING => {}
        // ---- playlist commands delegate to PlaylistStore ----
        ClientCommand::PLAYLIST_ADD { youtube_url, collection_id, added_by } => {
            let youtube_id = extract_youtube_id(&youtube_url)
                .ok_or_else(|| "Invalid YouTube URL".to_string())?;
            
            match crate::metadata::fetch_metadata(&youtube_id).await {
                Ok(metadata) => {
                    let song = Song {
                        id: Uuid::new_v4().to_string(),
                        youtube_id: youtube_id.clone(),
                        title: metadata.title,
                        artist: metadata.artist,
                        duration: metadata.duration,
                        original_title: None,
                        original_artist: None,
                        original_duration: None,
                        thumbnail_url: metadata.thumbnail_url,
                        added_by: added_by.unwrap_or_else(|| "Guest".to_string()),
                        added_at: chrono::Utc::now().timestamp_millis(),
                        resolution_status: Some(ResolutionStatus::Resolved),
                        source: Some(SongSource::Youtube {
                            video_id: Some(youtube_id.clone()),
                            url: Some(youtube_url),
                        }),
                    };
                    let target_id = if collection_id.is_empty() {
                        playlists.get_or_create_default_collection()
                    } else {
                        collection_id
                    };
                    if !playlists.add_to_collection(&target_id, song) {
                        return Err("Collection not found".to_string());
                    }
                    // Sync snapshot into room state
                    state.write().sync_playlists(playlists.get_all());
                }
                Err(e) => {
                    log::error!("Failed to fetch metadata: {}", e);
                    return Err(format!("Failed to fetch song metadata: {}", e));
                }
            }
        }
        ClientCommand::PLAYLIST_REMOVE { song_id, collection_id } => {
            let removed_youtube_id = playlists.collection_song_youtube_id(&collection_id, &song_id);
            if !playlists.remove_from_collection(&collection_id, &song_id) {
                return Err("Song not found in collection".to_string());
            }
            let mut room_state = state.write();
            if let Some(youtube_id) = removed_youtube_id {
                room_state.stop_if_current_youtube_id(&youtube_id);
            }
            room_state.sync_playlists(playlists.get_all());
        }
        ClientCommand::PLAYLIST_TO_QUEUE { song_id, collection_id } => {
            if let Some(song) = playlists.clone_song_for_queue(&collection_id, &song_id) {
                crate::youtube::ensure_video_is_embeddable(&song.youtube_id).await?;
                let mut room_state = state.write();
                queue_song_if_embeddable(&mut room_state, song.clone(), true)?;
                history.record_song(&song);
            } else {
                return Err("Esta cancion aun no esta resuelta. Usa la busqueda de YouTube primero.".to_string());
            }
        }
        ClientCommand::CREATE_COLLECTION { name, visibility } => {
            playlists.create_collection(name, visibility);
            state.write().sync_playlists(playlists.get_all());
        }
        ClientCommand::DELETE_COLLECTION { collection_id } => {
            let removed_video_ids = playlists.collection_video_ids(&collection_id);
            if !playlists.delete_collection(&collection_id) {
                return Err("Collection not found".to_string());
            }
            let mut room_state = state.write();
            for youtube_id in removed_video_ids {
                if room_state.stop_if_current_youtube_id(&youtube_id) {
                    break;
                }
            }
            room_state.sync_playlists(playlists.get_all());
        }
        ClientCommand::RENAME_COLLECTION { collection_id, name } => {
            if !playlists.rename_collection(&collection_id, name) {
                return Err("Collection not found".to_string());
            }
            state.write().sync_playlists(playlists.get_all());
        }
        ClientCommand::SET_COLLECTION_VISIBILITY { collection_id, visibility } => {
            if !playlists.set_collection_visibility(&collection_id, visibility) {
                return Err("Collection not found".to_string());
            }
            state.write().sync_playlists(playlists.get_all());
        }
        ClientCommand::IMPORT_COLLECTION { data } => {
            playlists.import_collection(&data)
                .map_err(|e| format!("Import failed: {}", e))?;
            state.write().sync_playlists(playlists.get_all());
        }
    }
    
    emit_state(&app, &state)?;

    Ok(())
}

fn queue_song_if_embeddable(
    room_state: &mut crate::room_state::RoomState,
    song: Song,
    is_embeddable: bool,
) -> Result<(), String> {
    if !is_embeddable {
        return Err(crate::youtube::unplayable_video_message());
    }
    room_state.add_song(song);
    Ok(())
}

fn parse_duration_label(label: &str) -> u32 {
    let mut total = 0u32;
    for part in label.split(':') {
        let Ok(value) = part.trim().parse::<u32>() else {
            return 0;
        };
        total = total.saturating_mul(60).saturating_add(value);
    }
    total
}

/// Broadcast the room state to the frontend.
///
/// Emits two events deliberately:
///   `room_state_updated` — full state, including personal collections, for the
///                          host's own UI.
///   `room_state_public`  — personal collections stripped, for rebroadcast to
///                          guests over the data channel.
///
/// Keeping the filtered view on this side means the guest broadcast path never
/// receives private data in the first place. Filtering in the frontend, as this
/// previously did, left one `.filter()` standing between a guest and every
/// personal playlist.
fn emit_state(app: &AppHandle, state: &tauri::State<RoomStateManager>) -> Result<(), String> {
    app.emit("room_state_updated", state.clone_state())
        .map_err(|e| e.to_string())?;
    app.emit("room_state_public", state.clone_public_state())
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Broadcast only the player slice of the state.
///
/// The host player reports progress roughly every five seconds, and each report
/// previously cloned and serialised the *entire* RoomState — queue plus every
/// public collection — to every connected guest. With a large library that is
/// tens of kilobytes per tick, per guest, over WebRTC, on phones, to convey a
/// timestamp that moved.
///
/// `player` is a self-contained subtree, so patching it cannot desync anything
/// else. Structural changes (queue, collections) deliberately keep emitting the
/// full state: they are rare, and a patch protocol for them would need sequence
/// numbers and a resync path to be safe. See OPTIMIZATION.md #1.
fn emit_player_patch(
    app: &AppHandle,
    state: &tauri::State<RoomStateManager>,
) -> Result<(), String> {
    let player = state.clone_player();
    // The host UI still wants the full object; it is in-process, so the cost is
    // a clone rather than a serialise-and-transmit.
    app.emit("room_state_updated", state.clone_state())
        .map_err(|e| e.to_string())?;
    app.emit("room_player_patch", player)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Update player state (called from frontend YouTube player)
#[tauri::command]
pub fn update_player_state(
    status: Option<String>,
    current_time: Option<f64>,
    duration: Option<f64>,
    state: tauri::State<RoomStateManager>,
    app: AppHandle,
) -> Result<(), String> {
    let player_status = status.and_then(|s| match s.as_str() {
        "playing" => Some(PlayerStatus::Playing),
        "paused" => Some(PlayerStatus::Paused),
        "loading" => Some(PlayerStatus::Loading),
        "error" => Some(PlayerStatus::Error),
        "idle" => Some(PlayerStatus::Idle),
        _ => None,
    });
    
    state.write().update_player(player_status, current_time, duration);

    // Player ticks are by far the highest-frequency broadcast; patch instead of
    // resending the whole room.
    emit_player_patch(&app, &state)?;

    Ok(())
}

#[tauri::command]
pub fn set_auto_play_next(
    auto_play_next: bool,
    state: tauri::State<RoomStateManager>,
    app: AppHandle,
) -> Result<(), String> {
    state.write().set_auto_play_next(auto_play_next);
    emit_state(&app, &state)?;
    Ok(())
}

#[tauri::command]
pub async fn open_player_display(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(PLAYER_DISPLAY_WINDOW_LABEL) {
        window.show().map_err(|e| e.to_string())?;
        notify_player_display_opened(&app);
        if let Some(main_window) = app.get_webview_window("main") {
            main_window.set_focus().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    let mut builder = WebviewWindowBuilder::new(
        &app,
        PLAYER_DISPLAY_WINDOW_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .initialization_script("window.__KARAOKE_PLAYER_DISPLAY__ = true;")
    .title("FESTEJAR Player Display")
    .inner_size(1280.0, 720.0)
    .resizable(true)
    .decorations(true)
    .center();

    if let Some(main_window) = app.get_webview_window("main") {
        builder = builder.owner(&main_window).map_err(|e| e.to_string())?;
    }

    let window = builder
    .build()
    .map_err(|e| e.to_string())?;

    window.show().map_err(|e| e.to_string())?;
    notify_player_display_opened(&app);
    if let Some(main_window) = app.get_webview_window("main") {
        main_window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn get_session_history(history: tauri::State<SessionHistoryStore>) -> Vec<Song> {
    history.get_all()
}

#[tauri::command]
pub fn export_session_history(history: tauri::State<SessionHistoryStore>) -> Result<Option<String>, String> {
    history
        .export_current_session()
        .map(|path| path.map(|p| p.display().to_string()))
}

#[tauri::command]
pub fn close_player_display(app: AppHandle) -> Result<(), String> {
    notify_player_display_closed(&app);
    if let Some(window) = app.get_webview_window(PLAYER_DISPLAY_WINDOW_LABEL) {
        let _ = window.set_fullscreen(false);
        window.destroy().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn shutdown_host_session(
    state: tauri::State<RoomStateManager>,
    app: AppHandle,
) -> Result<(), String> {
    state.write().shutdown_host_session();
    notify_player_display_closed(&app);
    if let Some(window) = app.get_webview_window(PLAYER_DISPLAY_WINDOW_LABEL) {
        let _ = window.set_fullscreen(false);
        window.destroy().map_err(|e| e.to_string())?;
    }
    emit_state(&app, &state)?;
    Ok(())
}

#[tauri::command]
pub fn set_player_display_fullscreen(app: AppHandle, fullscreen: bool) -> Result<(), String> {
    let window = app
        .get_webview_window(PLAYER_DISPLAY_WINDOW_LABEL)
        .ok_or_else(|| "Player display window is not open".to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    window.set_fullscreen(fullscreen).map_err(|e| e.to_string())
}

fn notify_player_display_opened(app: &AppHandle) {
    let _ = app.emit_to("main", "player-display-opened", ());
}

fn notify_player_display_closed(app: &AppHandle) {
    let _ = app.emit_to("main", "player-display-closed", ());
    if let Some(main_window) = app.get_webview_window("main") {
        let _ = main_window.set_focus();
    }
}

/// Export a collection to JSON string
#[tauri::command]
pub fn export_collection(collection_id: String, playlists: tauri::State<PlaylistStore>) -> Result<String, String> {
    playlists.export_collection(&collection_id)
}

// ============================================================
// Standalone playlist commands (available in ALL modes)
// ============================================================

/// Get all playlists (for Guest Mode local playlists too)
#[tauri::command]
pub fn get_playlists(playlists: tauri::State<PlaylistStore>) -> Vec<PlaylistCollection> {
    playlists.get_all()
}

/// Create a collection (standalone)
#[tauri::command]
pub fn playlist_create_collection(
    name: String,
    visibility: Option<String>,
    playlists: tauri::State<PlaylistStore>,
) -> String {
    let vis = match visibility.as_deref() {
        Some("personal") => CollectionVisibility::Personal,
        _ => CollectionVisibility::Public,
    };
    playlists.create_collection(name, vis)
}

/// Delete a collection (standalone)
#[tauri::command]
pub fn playlist_delete_collection(
    collection_id: String,
    playlists: tauri::State<PlaylistStore>,
    state: tauri::State<RoomStateManager>,
    app: AppHandle,
) -> Result<(), String> {
    let removed_video_ids = playlists.collection_video_ids(&collection_id);
    if playlists.delete_collection(&collection_id) {
        {
            let mut room_state = state.write();
            for youtube_id in removed_video_ids {
                if room_state.stop_if_current_youtube_id(&youtube_id) {
                    break;
                }
            }
            room_state.sync_playlists(playlists.get_all());
        }
        emit_state(&app, &state)?;
        Ok(())
    } else {
        Err("Collection not found".into())
    }
}

/// Rename a collection (standalone)
#[tauri::command]
pub fn playlist_rename_collection(
    collection_id: String,
    name: String,
    playlists: tauri::State<PlaylistStore>,
) -> Result<(), String> {
    if playlists.rename_collection(&collection_id, name) {
        Ok(())
    } else {
        Err("Collection not found".into())
    }
}

/// Set collection visibility (standalone)
#[tauri::command]
pub fn playlist_set_visibility(
    collection_id: String,
    visibility: String,
    playlists: tauri::State<PlaylistStore>,
) -> Result<(), String> {
    let vis = match visibility.as_str() {
        "personal" => CollectionVisibility::Personal,
        _ => CollectionVisibility::Public,
    };
    if playlists.set_collection_visibility(&collection_id, vis) {
        Ok(())
    } else {
        Err("Collection not found".into())
    }
}

/// Add a song to a collection (standalone, fetches metadata)
#[tauri::command]
pub async fn playlist_add_song(
    youtube_url: String,
    collection_id: String,
    added_by: Option<String>,
    playlists: tauri::State<'_, PlaylistStore>,
) -> Result<(), String> {
    let youtube_id = extract_youtube_id(&youtube_url)
        .ok_or_else(|| "Invalid YouTube URL".to_string())?;
    let metadata = crate::metadata::fetch_metadata(&youtube_id).await
        .map_err(|e| format!("Failed to fetch metadata: {}", e))?;
    let song = Song {
        id: Uuid::new_v4().to_string(),
        youtube_id: youtube_id.clone(),
        title: metadata.title,
        artist: metadata.artist,
        duration: metadata.duration,
        original_title: None,
        original_artist: None,
        original_duration: None,
        thumbnail_url: metadata.thumbnail_url,
        added_by: added_by.unwrap_or_else(|| "Host".to_string()),
        added_at: chrono::Utc::now().timestamp_millis(),
        resolution_status: Some(ResolutionStatus::Resolved),
        source: Some(SongSource::Youtube {
            video_id: Some(youtube_id),
            url: Some(youtube_url),
        }),
    };
    let target_id = if collection_id.is_empty() {
        playlists.get_or_create_default_collection()
    } else {
        collection_id
    };
    if playlists.add_to_collection(&target_id, song) {
        Ok(())
    } else {
        Err("Collection not found".into())
    }
}

/// Remove a song from a collection (standalone)
#[tauri::command]
pub fn playlist_remove_song(
    collection_id: String,
    song_id: String,
    playlists: tauri::State<PlaylistStore>,
    state: tauri::State<RoomStateManager>,
    app: AppHandle,
) -> Result<(), String> {
    let removed_youtube_id = playlists.collection_song_youtube_id(&collection_id, &song_id);
    if playlists.remove_from_collection(&collection_id, &song_id) {
        {
            let mut room_state = state.write();
            if let Some(youtube_id) = removed_youtube_id {
                room_state.stop_if_current_youtube_id(&youtube_id);
            }
            room_state.sync_playlists(playlists.get_all());
        }
        emit_state(&app, &state)?;
        Ok(())
    } else {
        Err("Song not found in collection".into())
    }
}

#[tauri::command]
pub async fn playlist_resolve_song(
    collection_id: String,
    song_id: String,
    result: crate::youtube::SearchResult,
    playlists: tauri::State<'_, PlaylistStore>,
    state: tauri::State<'_, RoomStateManager>,
    app: AppHandle,
) -> Result<(), String> {
    crate::youtube::ensure_video_is_embeddable(&result.id).await?;
    let existing = playlists
        .clone_song_from_collection(&collection_id, &song_id)
        .ok_or_else(|| "Song not found in collection".to_string())?;
    let original_title = Some(existing.original_title_or_current());
    let original_artist = Some(existing.original_artist_or_current());
    let original_duration = Some(existing.original_duration_or_current());
    let song = Song {
        id: song_id.clone(),
        youtube_id: result.id.clone(),
        title: original_title.clone().unwrap_or(result.title),
        artist: original_artist.clone().unwrap_or(result.channel),
        duration: original_duration.unwrap_or_else(|| parse_duration_label(&result.duration)),
        original_title,
        original_artist,
        original_duration,
        thumbnail_url: result.thumbnail,
        added_by: existing.added_by,
        added_at: existing.added_at,
        resolution_status: Some(ResolutionStatus::Resolved),
        source: Some(SongSource::Youtube {
            video_id: Some(result.id),
            url: Some(result.url),
        }),
    };
    if !playlists.replace_song_in_collection(&collection_id, &song_id, song) {
        return Err("Song not found in collection".into());
    }
    state.write().sync_playlists(playlists.get_all());
    emit_state(&app, &state)?;
    Ok(())
}

#[tauri::command]
pub fn playlist_queue_collection(
    collection_id: String,
    added_by: Option<String>,
    playlists: tauri::State<PlaylistStore>,
    state: tauri::State<RoomStateManager>,
    history: tauri::State<SessionHistoryStore>,
    app: AppHandle,
) -> Result<usize, String> {
    let added_by = added_by.unwrap_or_else(|| "Host".to_string());
    let songs = playlists
        .clone_collection_for_queue(&collection_id, &added_by)
        .ok_or_else(|| "Collection not found".to_string())?;
    if songs.is_empty() {
        return Err("No hay canciones resueltas para encolar.".to_string());
    }
    {
        let mut room_state = state.write();
        for song in &songs {
            room_state.add_song(song.clone());
            history.record_song(song);
        }
    }
    emit_state(&app, &state)?;
    Ok(songs.len())
}

#[tauri::command]
pub fn playlist_move_songs(
    source_collection_id: String,
    target_collection_id: String,
    song_ids: Vec<String>,
    playlists: tauri::State<PlaylistStore>,
    state: tauri::State<RoomStateManager>,
    app: AppHandle,
) -> Result<(), String> {
    if !playlists.move_songs_to_collection(&source_collection_id, &target_collection_id, &song_ids) {
        return Err("No songs were moved".into());
    }
    state.write().sync_playlists(playlists.get_all());
    emit_state(&app, &state)?;
    Ok(())
}

/// Import a collection from JSON string (standalone)
#[tauri::command]
pub fn playlist_import_collection(
    data: String,
    playlists: tauri::State<PlaylistStore>,
) -> Result<String, String> {
    playlists.import_collection(&data)
}

#[tauri::command]
pub async fn preview_spotify_playlist_import(
    url: String,
    playlists: tauri::State<'_, PlaylistStore>,
) -> Result<ImportPreview, String> {
    let client = SpotifyHttpClient::new()?;
    let (playlist_id, embed_url, html) = client.fetch_embed_html(&url).await?;
    let importer = SpotifyPlaylistImporter::new(html, playlist_id.clone(), embed_url);
    let playlist = importer.import(&url)?;
    let existing_collection_id = playlists.find_collection_by_spotify_playlist_id(&playlist_id);
    Ok(preview_from_imported(playlist, existing_collection_id))
}

#[tauri::command]
pub fn preview_karaoke_json_playlist_import(data: String) -> Result<ImportPreview, String> {
    let playlist = KaraokeJsonImporter.import(&data)?;
    Ok(preview_from_imported(playlist, None))
}

#[tauri::command]
pub fn preview_text_playlist_import(data: String) -> Result<ImportPreview, String> {
    let playlist = TextPlaylistImporter.import(&data)?;
    Ok(preview_from_imported(playlist, None))
}

#[tauri::command]
pub fn confirm_playlist_import(
    playlist: ImportedPlaylist,
    update_existing: bool,
    playlists: tauri::State<PlaylistStore>,
    state: tauri::State<RoomStateManager>,
) -> Result<String, String> {
    let existing_id = spotify_playlist_id_from_import(&playlist)
        .and_then(|playlist_id| playlists.find_collection_by_spotify_playlist_id(playlist_id));
    let collection = imported_playlist_to_collection(playlist, existing_id);
    let id = playlists.upsert_imported_collection(collection, update_existing)?;
    state.write().sync_playlists(playlists.get_all());
    Ok(id)
}

/// Human-readable description of a dialog-returned `FilePath`, for logs and
/// error messages. Handles both variants explicitly — never `.unwrap()`s —
/// because on Android the dialog returns `FilePath::Url` (a `content://`
/// URI), which has no filesystem path at all (`as_path()` is `None`).
fn describe_file_path(path: &tauri_plugin_fs::FilePath) -> String {
    match path {
        tauri_plugin_fs::FilePath::Path(p) => p.display().to_string(),
        tauri_plugin_fs::FilePath::Url(u) => u.to_string(),
    }
}

/// Build a descriptive error for an I/O failure against a dialog-returned
/// target, without needing the `FilePath` (and thus an `AppHandle`) in scope
/// — kept separate from `describe_file_path` so it's trivially unit-testable.
fn describe_io_error(action: &str, target: &str, err: &std::io::Error) -> String {
    format!("Failed to {action} {target}: {err}")
}

/// Save a collection to a file (using system file dialog)
///
/// Uses `tauri_plugin_fs`'s `Fs` API (via `FsExt`) instead of converting the
/// dialog's `FilePath` to a plain filesystem path: on Android the dialog can
/// return a `content://` URI, which `as_path()` cannot resolve and which
/// `into_path()` also can't turn into a real path (it only handles `file://`
/// URLs). `Fs::open` handles both the desktop path case and the Android
/// content-URI case (via the platform's `ContentResolver`), so it's the
/// right layer to read/write through rather than reimplementing that here.
#[tauri::command]
pub async fn save_collection_to_file(
    collection_id: String,
    playlists: tauri::State<'_, PlaylistStore>,
    app: AppHandle,
) -> Result<(), String> {
    use std::io::Write;
    use tauri_plugin_dialog::DialogExt;
    use tauri_plugin_fs::{FsExt, OpenOptions};

    let json = playlists.export_collection(&collection_id)?;
    log::info!("Exporting collection {} (JSON length: {})", collection_id, json.len());

    // Get a suggested filename from collection name
    let all = playlists.get_all();
    let col_name = all.iter()
        .find(|c| c.id == collection_id)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "playlist".to_string());
    let safe_name = col_name.replace(|c: char| !c.is_alphanumeric() && c != ' ' && c != '-' && c != '_', "");

    let path = app.dialog()
        .file()
        .set_file_name(&format!("{}.karaoke.json", safe_name))
        .add_filter("FESTEJAR Playlist", &["karaoke.json", "json"])
        .blocking_save_file();

    let Some(file_path) = path else {
        return Err("Save cancelled".into());
    };

    let target = describe_file_path(&file_path);
    log::info!("Saving collection to: {}", target);

    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);

    let mut file = app
        .fs()
        .open(file_path, opts)
        .map_err(|e| describe_io_error("open for writing", &target, &e))?;

    file.write_all(json.as_bytes())
        .map_err(|e| describe_io_error("write", &target, &e))?;

    log::info!("Saved collection to: {} ({} bytes)", target, json.len());

    Ok(())
}

/// Load a collection from a file (using system file dialog)
///
/// See `save_collection_to_file` for why this goes through `tauri_plugin_fs`
/// instead of `FilePath::as_path()`/`std::fs`.
#[tauri::command]
pub async fn load_collection_from_file(
    playlists: tauri::State<'_, PlaylistStore>,
    app: AppHandle,
) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;
    use tauri_plugin_fs::FsExt;

    let path = app.dialog()
        .file()
        .add_filter("FESTEJAR Playlist", &["karaoke.json", "json"])
        .blocking_pick_file();

    let Some(file_path) = path else {
        return Err("Open cancelled".into());
    };

    let target = describe_file_path(&file_path);

    let data = app
        .fs()
        .read_to_string(file_path)
        .map_err(|e| describe_io_error("read", &target, &e))?;

    playlists.import_collection(&data)
}

// ============================================================
// Lazy host server start
// ============================================================

/// Start the web/signaling server (called when entering Host Mode)
#[tauri::command]
pub fn start_host_server() -> Result<u16, String> {
    if SERVER_STARTED.swap(true, Ordering::SeqCst) {
        // Already started — just return port
        return Ok(crate::web_server::get_server_port());
    }

    // Bind synchronously, on this thread, so the port is a fact before we
    // return it. The previous version spawned the server and slept 500ms
    // hoping the bind had landed: wasted latency on a fast machine, and a race
    // on a slow one, where get_qr_url could be called against a port that was
    // not listening yet.
    let (listener, port) = crate::web_server::bind_web_server().inspect_err(|_| {
        // Binding failed, so nothing is listening — let a later attempt retry
        // rather than latching the guard on a server that never started.
        SERVER_STARTED.store(false, Ordering::SeqCst);
    })?;

    // Serving is the async half, and it owns its own runtime on a dedicated
    // thread to keep the axum server off Tauri's executor.
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                // Previously an .expect() here panicked this thread silently.
                log::error!("[Tauri] Failed to create Tokio runtime: {}", e);
                return;
            }
        };
        rt.block_on(async {
            log::info!("[Tauri] Serving embedded web server on port {}", port);
            if let Err(e) = crate::web_server::serve_web_server(listener).await {
                log::error!("[Tauri] Web server error: {}", e);
            }
        });
    });

    log::info!("[Tauri] Web server (and signaling) bound on port {}", port);
    Ok(port)
}

// ============================================================
// Diagnostics
// ============================================================

/// URL used by `report_issue`.
const GITHUB_REPOSITORY_URL: &str = "https://github.com/AndreasIBZ/FESTEJAR";
const ISSUE_TRACKER_URL: &str = "https://github.com/AndreasIBZ/FESTEJAR/issues/new";
const APP_STORAGE_DIR_NAME: &str = "FESTEJAR";

pub fn festejar_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        app.path()
            .app_local_data_dir()
            .map_err(|e| format!("Could not resolve the data directory: {}", e))
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        dirs::data_local_dir()
            .or_else(dirs::data_dir)
            .map(|base| base.join(APP_STORAGE_DIR_NAME).join("data"))
            .ok_or_else(|| "Could not resolve the local data directory".to_string())
    }
}

#[derive(Debug, Serialize)]
pub struct AppSettingsInfo {
    #[serde(rename = "productName")]
    pub product_name: String,
    pub version: String,
    #[serde(rename = "dataDir")]
    pub data_dir: Option<String>,
    #[serde(rename = "logDir")]
    pub log_dir: Option<String>,
    #[serde(rename = "playlistsPath")]
    pub playlists_path: Option<String>,
    #[serde(rename = "sessionHistoryDir")]
    pub session_history_dir: Option<String>,
}

/// Return the user-facing app metadata and storage locations shown in Settings.
#[tauri::command]
pub fn get_app_settings_info(app: AppHandle) -> AppSettingsInfo {
    let data_dir = festejar_data_dir(&app).ok();
    let log_dir = app.path().app_log_dir().ok();
    let playlists_path = data_dir.as_ref().map(|dir| dir.join("playlists.json"));
    let session_history_dir = data_dir.as_ref().map(|dir| dir.join("session-history"));

    AppSettingsInfo {
        product_name: "FESTEJAR".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        data_dir: data_dir.map(|path| path.display().to_string()),
        log_dir: log_dir.map(|path| path.display().to_string()),
        playlists_path: playlists_path.map(|path| path.display().to_string()),
        session_history_dir: session_history_dir.map(|path| path.display().to_string()),
    }
}

/// Reveal the app data directory. This contains `playlists.json` and
/// `session-history/`.
#[tauri::command]
pub fn open_data_folder(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        Err("Opening the data folder is not supported on Android.".to_string())
    }

    #[cfg(not(target_os = "android"))]
    {
        let dir = festejar_data_dir(&app)?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create the data directory: {}", e))?;
        open_path(&dir)
    }
}

/// Reveal the automatically exported session-history directory.
#[tauri::command]
pub fn open_session_history_folder(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        Err("Opening the session history folder is not supported on Android.".to_string())
    }

    #[cfg(not(target_os = "android"))]
    {
        let dir = festejar_data_dir(&app)?.join("session-history");
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create the session history directory: {}", e))?;
        open_path(&dir)
    }
}

/// Reveal the directory containing the app's log files in the OS file manager.
///
/// Desktop only. Android has no user-visible file manager entry point for the
/// app-private log directory, so this reports a clear error rather than
/// pretending to succeed.
#[tauri::command]
pub fn open_log_folder(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let _ = app;
        Err("Opening the log folder is not supported on Android.".to_string())
    }

    #[cfg(not(target_os = "android"))]
    {
        use tauri::Manager;

        let dir = app
            .path()
            .app_log_dir()
            .map_err(|e| format!("Could not resolve the log directory: {}", e))?;

        // The directory does not exist until the logger first writes to it.
        if !dir.exists() {
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("Could not create the log directory: {}", e))?;
        }

        open_path(&dir)
    }
}

/// Open the issue tracker in the user's default browser.
#[tauri::command]
pub fn report_issue(app: AppHandle) -> Result<(), String> {
    let _ = &app;
    open_url(ISSUE_TRACKER_URL)
}

/// Open the public GitHub repository in the user's default browser.
#[tauri::command]
pub fn open_github_repository(app: AppHandle) -> Result<(), String> {
    let _ = &app;
    open_url(GITHUB_REPOSITORY_URL)
}

/// Open a filesystem path with the platform's file manager.
#[cfg(not(target_os = "android"))]
fn open_path(path: &std::path::Path) -> Result<(), String> {
    let path_str = path.to_str().ok_or("Path is not valid UTF-8")?;
    spawn_opener(path_str)
}

/// Open a URL with the platform's default handler.
fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        // No process spawning on Android; the webview handles navigation.
        Err(format!("Open this URL manually: {}", url))
    }

    #[cfg(not(target_os = "android"))]
    {
        spawn_opener(url)
    }
}

/// Hand a path or URL to the platform opener.
///
/// Deliberately not `tauri-plugin-shell`: that plugin is declared in
/// package.json but present in neither Cargo.toml nor the capability file, so
/// it does not actually work here. This spawns the OS opener directly and is
/// compiled out on Android, where process spawning is unavailable.
#[cfg(not(target_os = "android"))]
fn spawn_opener(target: &str) -> Result<(), String> {
    use std::process::Command;

    #[cfg(target_os = "windows")]
    let result = Command::new("cmd").args(["/C", "start", "", target]).spawn();

    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(target).spawn();

    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open").arg(target).spawn();

    result
        .map(|_| ())
        .map_err(|e| format!("Could not open '{}': {}", target, e))
}

/// Response types
#[derive(Debug, Serialize)]
pub struct CreateRoomResponse {
    pub room_id: String,
    pub join_token: String,
}

/// Generate a unique room ID
fn generate_room_id() -> String {
    use sha2::{Sha256, Digest};
    
    let uuid = Uuid::new_v4();
    let mut hasher = Sha256::new();
    hasher.update(uuid.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..3]) // 6 characters
}

/// Generate a secure join token
fn generate_join_token() -> String {
    Uuid::new_v4().to_string().replace("-", "")
}

/// Extract YouTube video ID from URL
fn extract_youtube_id(url: &str) -> Option<String> {
    // Support various YouTube URL formats
    // https://www.youtube.com/watch?v=VIDEO_ID
    // https://youtu.be/VIDEO_ID
    // youtube.com/watch?v=VIDEO_ID
    
    if let Some(pos) = url.find("v=") {
        let start = pos + 2;
        let end = url[start..].find('&').map(|p| start + p).unwrap_or(url.len());
        Some(url[start..end].to_string())
    } else if url.contains("youtu.be/") {
        if let Some(pos) = url.find("youtu.be/") {
            let start = pos + 9;
            let end = url[start..].find('?').map(|p| start + p).unwrap_or(url.len());
            Some(url[start..end].to_string())
        } else {
            None
        }
    } else {
        // Assume it's already a video ID
        if url.len() == 11 {
            Some(url.to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_song(id: &str, youtube_id: &str) -> Song {
        Song {
            id: id.to_string(),
            youtube_id: youtube_id.to_string(),
            title: format!("title-{id}"),
            artist: "artist".to_string(),
            duration: 180,
            original_title: None,
            original_artist: None,
            original_duration: None,
            thumbnail_url: "thumb.jpg".to_string(),
            added_by: "Guest".to_string(),
            added_at: 1,
            resolution_status: Some(ResolutionStatus::Resolved),
            source: Some(SongSource::Youtube {
                video_id: Some(youtube_id.to_string()),
                url: None,
            }),
        }
    }

    fn room_state_with_current_song() -> crate::room_state::RoomState {
        let mut room_state = crate::room_state::RoomState::new(
            "room".to_string(),
            "host".to_string(),
            Vec::new(),
        );
        room_state.player.current_song = Some(dummy_song("current", "currentid"));
        room_state
    }

    #[test]
    fn test_queue_song_if_embeddable_adds_to_queue() {
        let mut room_state = room_state_with_current_song();
        let before_len = room_state.queue.len();

        queue_song_if_embeddable(&mut room_state, dummy_song("next", "abc123"), true)
            .expect("embeddable song should enter the queue");

        assert_eq!(room_state.queue.len(), before_len + 1);
        assert_eq!(room_state.queue[0].youtube_id, "abc123");
    }

    #[test]
    fn test_queue_song_if_not_embeddable_leaves_queue_unchanged() {
        let mut room_state = room_state_with_current_song();
        let before_len = room_state.queue.len();

        let result = queue_song_if_embeddable(&mut room_state, dummy_song("bad", "blockedid"), false);

        assert_eq!(result, Err(crate::youtube::unplayable_video_message()));
        assert_eq!(room_state.queue.len(), before_len);
        assert!(room_state.queue.iter().all(|song| song.youtube_id != "blockedid"));
    }

    #[test]
    fn test_extract_youtube_id() {
        assert_eq!(
            extract_youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            extract_youtube_id("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            extract_youtube_id("dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_parse_duration_label() {
        assert_eq!(parse_duration_label("3:45"), 225);
        assert_eq!(parse_duration_label("1:02:03"), 3723);
        assert_eq!(parse_duration_label("Live"), 0);
    }

    /// T13 regression coverage: `describe_file_path` must handle both
    /// `FilePath` variants without panicking. The bug this fixes was an
    /// `.unwrap()` on `FilePath::as_path()`, which is `None` for the
    /// `FilePath::Url` variant Android's file dialog returns — that call
    /// would have panicked here too if the same pattern were used.
    #[test]
    fn test_describe_file_path_handles_plain_paths() {
        let path = tauri_plugin_fs::FilePath::Path(std::path::PathBuf::from("/tmp/playlist.karaoke.json"));
        assert_eq!(describe_file_path(&path), "/tmp/playlist.karaoke.json");
    }

    #[test]
    fn test_describe_file_path_handles_content_uris_without_panicking() {
        // Shape of what Android's SAF file picker actually returns — not a
        // filesystem path, so `FilePath::as_path()` is `None` and the old
        // `.unwrap()` would panic on exactly this input.
        let url = url::Url::parse(
            "content://com.android.externalstorage.documents/document/primary%3ADownload%2Fplaylist.json",
        )
        .expect("valid content URI");
        let path = tauri_plugin_fs::FilePath::Url(url.clone());

        assert_eq!(path.as_path(), None, "content URIs have no filesystem path");
        assert_eq!(describe_file_path(&path), url.to_string());
    }

    #[test]
    fn test_describe_io_error_includes_action_and_target() {
        let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let msg = describe_io_error("open for writing", "content://example/doc", &err);
        assert!(msg.contains("open for writing"));
        assert!(msg.contains("content://example/doc"));
        assert!(msg.contains("denied"));
    }
}
