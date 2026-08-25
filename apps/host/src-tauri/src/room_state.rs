use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::path::PathBuf;
use std::fs;

/// Load playlists from file
fn load_playlists_from_file(path: &PathBuf) -> Vec<PlaylistCollection> {
    if path.exists() {
        match fs::read_to_string(path) {
            Ok(content) => {
                match serde_json::from_str::<Vec<PlaylistCollection>>(&content) {
                    Ok(playlists) => {
                        let total_songs: usize = playlists.iter().map(|c| c.songs.len()).sum();
                        log::info!("Loaded {} collections ({} total songs) from playlists file: {:?}", playlists.len(), total_songs, path);
                        return playlists;
                    }
                    Err(e) => log::error!("Failed to parse playlists file {:?}: {}", path, e),
                }
            }
            Err(e) => log::error!("Failed to read playlists file {:?}: {}", path, e),
        }
    }
    Vec::new()
}

/// Save playlists to file
fn save_playlists_to_file(path: &PathBuf, playlists: &[PlaylistCollection]) {
    match serde_json::to_string_pretty(playlists) {
        Ok(content) => {
            if let Err(e) = fs::write(path, content) {
                log::error!("Failed to write playlists file {:?}: {}", path, e);
            } else {
                let total_songs: usize = playlists.iter().map(|c| c.songs.len()).sum();
                log::info!("Saved {} collections ({} total songs) to playlists file: {:?}", playlists.len(), total_songs, path);
            }
        }
        Err(e) => log::error!("Failed to serialize playlists: {}", e),
    }
}

/// Represents a song in the queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Song {
    pub id: String,
    #[serde(rename = "youtubeId")]
    #[serde(default)]
    pub youtube_id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub duration: u32,
    #[serde(default, rename = "originalTitle")]
    pub original_title: Option<String>,
    #[serde(default, rename = "originalArtist")]
    pub original_artist: Option<String>,
    #[serde(default, rename = "originalDuration")]
    pub original_duration: Option<u32>,
    #[serde(rename = "thumbnailUrl")]
    #[serde(default)]
    pub thumbnail_url: String,
    #[serde(rename = "addedBy")]
    #[serde(default)]
    pub added_by: String,
    #[serde(rename = "addedAt")]
    #[serde(default)]
    pub added_at: i64,
    #[serde(default, rename = "resolutionStatus")]
    pub resolution_status: Option<ResolutionStatus>,
    #[serde(default)]
    pub source: Option<SongSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResolutionStatus {
    Resolved,
    Unresolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum SongSource {
    Local,
    Spotify {
        #[serde(default, rename = "playlistId")]
        playlist_id: Option<String>,
        #[serde(default, rename = "trackId")]
        track_id: Option<String>,
        #[serde(default)]
        url: Option<String>,
    },
    Youtube {
        #[serde(default, rename = "videoId")]
        video_id: Option<String>,
        #[serde(default)]
        url: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum PlaylistSource {
    Local,
    Spotify {
        #[serde(rename = "playlistId")]
        playlist_id: String,
        #[serde(rename = "originalUrl")]
        original_url: String,
        #[serde(rename = "importedAt")]
        imported_at: i64,
    },
}

impl Song {
    pub fn original_title_or_current(&self) -> String {
        self.original_title
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| self.title.clone())
    }

    pub fn original_artist_or_current(&self) -> String {
        self.original_artist
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| self.artist.clone())
    }

    pub fn original_duration_or_current(&self) -> u32 {
        self.original_duration.unwrap_or(self.duration)
    }
}

/// Collection visibility
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CollectionVisibility {
    Public,
    Personal,
}

/// A named collection of songs (playlist)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistCollection {
    pub id: String,
    pub name: String,
    pub visibility: CollectionVisibility,
    pub songs: Vec<Song>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub source: Option<PlaylistSource>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

// ============================================================
// PlaylistStore — standalone, decoupled from Room lifecycle
// ============================================================

/// Thread-safe playlist store (always available, both Host and Guest modes)
pub struct PlaylistStore {
    base_path: Arc<RwLock<Option<PathBuf>>>,
    playlists: Arc<RwLock<Vec<PlaylistCollection>>>,
}

impl PlaylistStore {
    pub fn new() -> Self {
        Self {
            base_path: Arc::new(RwLock::new(None)),
            playlists: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Initialize the store with a persistent path and load data
    pub fn initialize(&self, app_data_dir: PathBuf) -> Vec<PlaylistCollection> {
        let mut path = app_data_dir;
        let _ = fs::create_dir_all(&path);
        path.push("playlists.json");
        
        // 1. Check if new path exists
        if !path.exists() {
            // 2. Try migration from old 'KaraokeNatin' dir if on desktop
            #[cfg(not(target_os = "android"))]
            {
                let old_base = dirs::data_local_dir()
                    .or_else(|| dirs::data_dir());
                
                if let Some(mut old_path) = old_base {
                    old_path.push("KaraokeNatin");
                    
                    // Try migrating playlists.json
                    let mut old_playlists_json = old_path.clone();
                    old_playlists_json.push("playlists.json");
                    
                    if old_playlists_json.exists() {
                        log::info!("Migrating existing playlists.json from {:?}", old_playlists_json);
                        if let Err(e) = fs::copy(&old_playlists_json, &path) {
                            log::error!(
                                "Failed to migrate playlists.json from {:?} to {:?}: {}",
                                old_playlists_json, path, e
                            );
                        }
                        // Optionally remove if you want to be clean, but maybe safer to keep for now
                        // let _ = fs::remove_file(&old_playlists_json);
                    } else {
                        // Try migrating legacy playlist.json
                        let mut legacy_json = old_path.clone();
                        legacy_json.push("playlist.json");
                        
                        if legacy_json.exists() {
                            log::info!("Migrating legacy playlist.json from {:?}", legacy_json);
                            if let Ok(content) = fs::read_to_string(&legacy_json) {
                                if let Ok(songs) = serde_json::from_str::<Vec<Song>>(&content) {
                                    let now = chrono::Utc::now().timestamp_millis();
                                    let default_collection = PlaylistCollection {
                                        id: uuid::Uuid::new_v4().to_string(),
                                        name: "Default Playlist".to_string(),
                                        visibility: CollectionVisibility::Public,
                                        songs,
                                        description: None,
                                        source: Some(PlaylistSource::Local),
                                        created_at: now,
                                        updated_at: now,
                                    };
                                    save_playlists_to_file(&path, &[default_collection]);
                                    // let _ = fs::remove_file(&legacy_json);
                                }
                            }
                        }
                    }
                }
            }
        }

        let loaded_playlists = load_playlists_from_file(&path);
        
        *self.base_path.write() = Some(path);
        *self.playlists.write() = loaded_playlists.clone();
        
        loaded_playlists
    }

    fn save(&self) {
        let pl = self.playlists.read();
        if let Some(path) = self.base_path.read().as_ref() {
            save_playlists_to_file(path, &pl);
        }
    }

    /// Get a snapshot of all playlists
    pub fn get_all(&self) -> Vec<PlaylistCollection> {
        self.playlists.read().clone()
    }

    pub fn find_collection_by_spotify_playlist_id(&self, playlist_id: &str) -> Option<String> {
        self.playlists
            .read()
            .iter()
            .find_map(|collection| match &collection.source {
                Some(PlaylistSource::Spotify { playlist_id: existing_id, .. }) if existing_id == playlist_id => {
                    Some(collection.id.clone())
                }
                _ => None,
            })
    }

    pub fn upsert_imported_collection(&self, mut collection: PlaylistCollection, update_existing: bool) -> Result<String, String> {
        if let Some(PlaylistSource::Spotify { playlist_id, .. }) = &collection.source {
            if let Some(existing_id) = self.find_collection_by_spotify_playlist_id(playlist_id) {
                if !update_existing {
                    return Err("Esta playlist ya esta importada.".to_string());
                }
                collection.id = existing_id.clone();
                let replaced = {
                    let mut playlists = self.playlists.write();
                    if let Some(existing) = playlists.iter_mut().find(|c| c.id == existing_id) {
                        collection.created_at = existing.created_at;
                        *existing = collection;
                        true
                    } else {
                        false
                    }
                };
                if replaced {
                    self.save();
                    return Ok(existing_id);
                }
                return Err("Collection not found".to_string());
            }
        }

        let id = collection.id.clone();
        {
            let mut playlists = self.playlists.write();
            playlists.push(collection);
        }
        self.save();
        Ok(id)
    }

    /// Create a new playlist collection, returns its ID
    pub fn create_collection(&self, name: String, visibility: CollectionVisibility) -> String {
        let now = chrono::Utc::now().timestamp_millis();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut pl = self.playlists.write();
            pl.push(PlaylistCollection {
                id: id.clone(),
                name,
                visibility,
                songs: Vec::new(),
                description: None,
                source: Some(PlaylistSource::Local),
                created_at: now,
                updated_at: now,
            });
        }
        self.save();
        id
    }

    /// Delete a playlist collection
    pub fn delete_collection(&self, collection_id: &str) -> bool {
        let success = {
            let mut pl = self.playlists.write();
            if let Some(pos) = pl.iter().position(|c| c.id == collection_id) {
                pl.remove(pos);
                true
            } else {
                false
            }
        };
        if success {
            self.save();
        }
        success
    }

    pub fn collection_video_ids(&self, collection_id: &str) -> Vec<String> {
        self.playlists
            .read()
            .iter()
            .find(|c| c.id == collection_id)
            .map(|c| c.songs.iter().map(|s| s.youtube_id.clone()).collect())
            .unwrap_or_default()
    }

    /// Rename a playlist collection
    pub fn rename_collection(&self, collection_id: &str, name: String) -> bool {
        let success = {
            let mut pl = self.playlists.write();
            if let Some(col) = pl.iter_mut().find(|c| c.id == collection_id) {
                col.name = name;
                col.updated_at = chrono::Utc::now().timestamp_millis();
                true
            } else {
                false
            }
        };
        if success {
            self.save();
        }
        success
    }

    /// Set visibility of a playlist collection
    pub fn set_collection_visibility(&self, collection_id: &str, visibility: CollectionVisibility) -> bool {
        let success = {
            let mut pl = self.playlists.write();
            if let Some(col) = pl.iter_mut().find(|c| c.id == collection_id) {
                col.visibility = visibility;
                col.updated_at = chrono::Utc::now().timestamp_millis();
                true
            } else {
                false
            }
        };
        if success {
            self.save();
        }
        success
    }

    /// Add a song to a specific collection
    pub fn add_to_collection(&self, collection_id: &str, song: Song) -> bool {
        let success = {
            let mut pl = self.playlists.write();
            if let Some(col) = pl.iter_mut().find(|c| c.id == collection_id) {
                col.songs.push(song);
                col.updated_at = chrono::Utc::now().timestamp_millis();
                true
            } else {
                false
            }
        };
        if success {
            self.save();
        }
        success
    }

    /// Remove a song from a specific collection
    pub fn remove_from_collection(&self, collection_id: &str, song_id: &str) -> bool {
        let success = {
            let mut pl = self.playlists.write();
            if let Some(col) = pl.iter_mut().find(|c| c.id == collection_id) {
                if let Some(pos) = col.songs.iter().position(|s| s.id == song_id) {
                    col.songs.remove(pos);
                    col.updated_at = chrono::Utc::now().timestamp_millis();
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if success {
            self.save();
        }
        success
    }

    pub fn replace_song_in_collection(&self, collection_id: &str, song_id: &str, mut song: Song) -> bool {
        let success = {
            let mut playlists = self.playlists.write();
            if let Some(collection) = playlists.iter_mut().find(|c| c.id == collection_id) {
                if let Some(existing) = collection.songs.iter_mut().find(|s| s.id == song_id) {
                    song.id = existing.id.clone();
                    song.added_at = existing.added_at;
                    *existing = song;
                    collection.updated_at = chrono::Utc::now().timestamp_millis();
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if success {
            self.save();
        }
        success
    }

    pub fn clone_song_from_collection(&self, collection_id: &str, song_id: &str) -> Option<Song> {
        self.playlists
            .read()
            .iter()
            .find(|c| c.id == collection_id)
            .and_then(|c| c.songs.iter().find(|s| s.id == song_id))
            .cloned()
    }

    pub fn move_songs_to_collection(&self, source_collection_id: &str, target_collection_id: &str, song_ids: &[String]) -> bool {
        if source_collection_id == target_collection_id || song_ids.is_empty() {
            return true;
        }

        let success = {
            let mut playlists = self.playlists.write();
            let Some(source_index) = playlists.iter().position(|c| c.id == source_collection_id) else {
                return false;
            };
            let Some(target_index) = playlists.iter().position(|c| c.id == target_collection_id) else {
                return false;
            };
            let ids: std::collections::HashSet<&str> = song_ids.iter().map(String::as_str).collect();
            let mut moved = Vec::new();
            playlists[source_index].songs.retain(|song| {
                if ids.contains(song.id.as_str()) {
                    moved.push(song.clone());
                    false
                } else {
                    true
                }
            });
            if moved.is_empty() {
                return false;
            }
            let now = chrono::Utc::now().timestamp_millis();
            playlists[source_index].updated_at = now;
            playlists[target_index].songs.extend(moved);
            playlists[target_index].updated_at = now;
            true
        };

        if success {
            self.save();
        }
        success
    }

    pub fn collection_song_youtube_id(&self, collection_id: &str, song_id: &str) -> Option<String> {
        self.playlists
            .read()
            .iter()
            .find(|c| c.id == collection_id)
            .and_then(|c| c.songs.iter().find(|s| s.id == song_id))
            .and_then(|s| if s.youtube_id.is_empty() { None } else { Some(s.youtube_id.clone()) })
    }

    /// Copy a resolved YouTube song from a collection, returning a new Song for the queue.
    pub fn clone_song_for_queue(&self, collection_id: &str, song_id: &str) -> Option<Song> {
        let pl = self.playlists.read();
        pl.iter()
            .find(|c| c.id == collection_id)
            .and_then(|c| c.songs.iter().find(|s| s.id == song_id))
            .filter(|song| !song.youtube_id.is_empty())
            .map(|song| Song {
                id: uuid::Uuid::new_v4().to_string(),
                youtube_id: song.youtube_id.clone(),
                title: song.title.clone(),
                artist: song.artist.clone(),
                duration: song.duration,
                original_title: song.original_title.clone(),
                original_artist: song.original_artist.clone(),
                original_duration: song.original_duration,
                thumbnail_url: song.thumbnail_url.clone(),
                added_by: song.added_by.clone(),
                added_at: chrono::Utc::now().timestamp_millis(),
                resolution_status: Some(ResolutionStatus::Resolved),
                source: song.source.clone(),
            })
    }

    pub fn clone_collection_for_queue(&self, collection_id: &str, added_by: &str) -> Option<Vec<Song>> {
        let pl = self.playlists.read();
        let collection = pl.iter().find(|c| c.id == collection_id)?;
        let now = chrono::Utc::now().timestamp_millis();
        let songs = collection
            .songs
            .iter()
            .filter(|song| !song.youtube_id.is_empty())
            .map(|song| Song {
                id: uuid::Uuid::new_v4().to_string(),
                youtube_id: song.youtube_id.clone(),
                title: song.title.clone(),
                artist: song.artist.clone(),
                duration: song.duration,
                original_title: song.original_title.clone(),
                original_artist: song.original_artist.clone(),
                original_duration: song.original_duration,
                thumbnail_url: song.thumbnail_url.clone(),
                added_by: added_by.to_string(),
                added_at: now,
                resolution_status: Some(ResolutionStatus::Resolved),
                source: song.source.clone(),
            })
            .collect();
        Some(songs)
    }

    /// Get the default collection ID, creating one if none exist
    pub fn get_or_create_default_collection(&self) -> String {
        {
            let pl = self.playlists.read();
            if let Some(first) = pl.first() {
                return first.id.clone();
            }
        }
        self.create_collection("Default Playlist".to_string(), CollectionVisibility::Public)
    }

    /// Import a collection from JSON data
    pub fn import_collection(&self, data: &str) -> Result<String, String> {
        #[derive(Deserialize)]
        struct ExportedCollection {
            #[allow(dead_code)]
            karaokenatin: Option<String>,
            #[allow(dead_code)]
            format: Option<String>,
            #[allow(dead_code)]
            version: Option<u32>,
            collection: ImportedCollectionData,
        }
        #[derive(Deserialize)]
        struct ImportedCollectionData {
            name: String,
            visibility: CollectionVisibility,
            songs: Vec<Song>,
        }

        let exported: ExportedCollection = serde_json::from_str(data)
            .map_err(|e| format!("Invalid collection data: {}", e))?;
        if exported.karaokenatin.is_none() && exported.format.as_deref() != Some("karaoke-playlist") {
            return Err("El archivo no es una playlist compatible con FESTEJAR.".to_string());
        }
        if matches!(exported.version, Some(version) if version != 1) {
            return Err("Version de playlist no soportada.".to_string());
        }
        
        let now = chrono::Utc::now().timestamp_millis();
        let id = uuid::Uuid::new_v4().to_string();
        
        let songs: Vec<Song> = exported.collection.songs.into_iter().map(|mut s| {
            s.id = uuid::Uuid::new_v4().to_string();
            s.added_at = now;
            s
        }).collect();
        
        {
            let mut pl = self.playlists.write();
            
            // Check for name collision and add suffix if needed
            let base_name = exported.collection.name.clone();
            let mut final_name = exported.collection.name;
            let mut suffix_count = 0;
            while pl.iter().any(|c| c.name == final_name) {
                suffix_count += 1;
                if suffix_count == 1 {
                    final_name = format!("{} (Imported)", final_name);
                } else {
                    final_name = format!("{} (Imported {})", base_name, suffix_count);
                }
            }

            pl.push(PlaylistCollection {
                id: id.clone(),
                name: final_name,
                visibility: exported.collection.visibility,
                songs,
                description: None,
                source: Some(PlaylistSource::Local),
                created_at: now,
                updated_at: now,
            });
        }
        self.save();
        Ok(id)
    }

    /// Export a collection to JSON
    pub fn export_collection(&self, collection_id: &str) -> Result<String, String> {
        let pl = self.playlists.read();
        let col = pl.iter().find(|c| c.id == collection_id)
            .ok_or_else(|| "Collection not found".to_string())?;
        
        #[derive(Serialize)]
        struct KaraokePlaylistFile<'a> {
            format: &'a str,
            version: u32,
            generator: &'a str,
            name: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            description: &'a Option<String>,
            source: KaraokePlaylistSource,
            tracks: Vec<KaraokePlaylistTrack>,
        }
        #[derive(Serialize)]
        struct KaraokePlaylistTrack {
            title: String,
            artists: Vec<String>,
            #[serde(skip_serializing_if = "Option::is_none", rename = "duration_ms")]
            duration_ms: Option<u32>,
            source: KaraokeTrackSource,
        }
        #[derive(Serialize)]
        struct KaraokePlaylistSource {
            #[serde(rename = "type")]
            source_type: String,
            #[serde(skip_serializing_if = "Option::is_none", rename = "playlist_id")]
            playlist_id: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none", rename = "original_url")]
            original_url: Option<String>,
        }
        #[derive(Serialize)]
        struct KaraokeTrackSource {
            #[serde(rename = "type")]
            source_type: String,
            #[serde(skip_serializing_if = "Option::is_none", rename = "playlist_id")]
            playlist_id: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none", rename = "track_id")]
            track_id: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            url: Option<String>,
        }

        let tracks = col.songs.iter().map(|song| {
            let source = match song.source.clone().unwrap_or(SongSource::Local) {
                SongSource::Spotify { playlist_id, track_id, url } => KaraokeTrackSource {
                    source_type: "spotify".to_string(),
                    playlist_id,
                    track_id,
                    url,
                },
                SongSource::Youtube { video_id, url } => KaraokeTrackSource {
                    source_type: "youtube".to_string(),
                    playlist_id: None,
                    track_id: video_id,
                    url,
                },
                SongSource::Local => KaraokeTrackSource {
                    source_type: "local".to_string(),
                    playlist_id: None,
                    track_id: None,
                    url: None,
                },
            };
            KaraokePlaylistTrack {
                title: song.title.clone(),
                artists: if song.artist.trim().is_empty() {
                    Vec::new()
                } else {
                    song.artist.split(',').map(|artist| artist.trim().to_string()).collect()
                },
                duration_ms: if song.duration == 0 { None } else { Some(song.duration * 1000) },
                source,
            }
        }).collect();
        let source = match col.source.clone().unwrap_or(PlaylistSource::Local) {
            PlaylistSource::Spotify { playlist_id, original_url, .. } => KaraokePlaylistSource {
                source_type: "spotify".to_string(),
                playlist_id: Some(playlist_id),
                original_url: Some(original_url),
            },
            PlaylistSource::Local => KaraokePlaylistSource {
                source_type: "local".to_string(),
                playlist_id: None,
                original_url: None,
            },
        };

        let export = KaraokePlaylistFile {
            format: "karaoke-playlist",
            version: 1,
            generator: "FESTEJAR",
            name: &col.name,
            description: &col.description,
            source,
            tracks,
        };
        
        serde_json::to_string_pretty(&export)
            .map_err(|e| format!("Failed to serialize: {}", e))
    }
}

// ============================================================
// SessionHistoryStore — current host session song history
// ============================================================

pub struct SessionHistoryStore {
    base_dir: Arc<RwLock<Option<PathBuf>>>,
    session_id: String,
    started_at: i64,
    exported_song_count: Arc<RwLock<usize>>,
    songs: Arc<RwLock<Vec<Song>>>,
}

impl SessionHistoryStore {
    pub fn new() -> Self {
        Self {
            base_dir: Arc::new(RwLock::new(None)),
            session_id: uuid::Uuid::new_v4().to_string(),
            started_at: chrono::Utc::now().timestamp_millis(),
            exported_song_count: Arc::new(RwLock::new(0)),
            songs: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub fn initialize(&self, app_data_dir: PathBuf) {
        let mut dir = app_data_dir;
        dir.push("session-history");
        if let Err(e) = fs::create_dir_all(&dir) {
            log::error!("Failed to create session history dir {:?}: {}", dir, e);
            return;
        }
        *self.base_dir.write() = Some(dir);
    }

    pub fn record_song(&self, song: &Song) {
        self.songs.write().push(song.clone());
    }

    pub fn get_all(&self) -> Vec<Song> {
        self.songs.read().clone()
    }

    pub fn export_current_session(&self) -> Result<Option<PathBuf>, String> {
        let songs = self.songs.read();
        if songs.is_empty() {
            return Ok(None);
        }
        if *self.exported_song_count.read() == songs.len() {
            return Ok(None);
        }

        let Some(base_dir) = self.base_dir.read().clone() else {
            return Ok(None);
        };

        let exported_at = chrono::Utc::now().timestamp_millis();
        let file_stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let path = base_dir.join(format!("karaoke-session-{}.karaoke.json", file_stamp));

        #[derive(Serialize)]
        struct ExportedSession<'a> {
            format: &'a str,
            version: u32,
            generator: &'a str,
            kind: &'a str,
            session: ExportedSessionMeta<'a>,
            collection: ExportedCollectionData<'a>,
        }
        #[derive(Serialize)]
        struct ExportedSessionMeta<'a> {
            id: &'a str,
            #[serde(rename = "startedAt")]
            started_at: i64,
            #[serde(rename = "exportedAt")]
            exported_at: i64,
            #[serde(rename = "songCount")]
            song_count: usize,
        }
        #[derive(Serialize)]
        struct ExportedCollectionData<'a> {
            name: String,
            visibility: CollectionVisibility,
            songs: &'a [Song],
        }

        let export = ExportedSession {
            format: "karaoke-playlist",
            version: 1,
            generator: "FESTEJAR",
            kind: "session-history",
            session: ExportedSessionMeta {
                id: &self.session_id,
                started_at: self.started_at,
                exported_at,
                song_count: songs.len(),
            },
            collection: ExportedCollectionData {
                name: format!("Session {}", file_stamp),
                visibility: CollectionVisibility::Personal,
                songs: &songs,
            },
        };

        let json = serde_json::to_string_pretty(&export)
            .map_err(|e| format!("Failed to serialize session history: {}", e))?;
        fs::write(&path, json)
            .map_err(|e| format!("Failed to write session history {:?}: {}", path, e))?;
        *self.exported_song_count.write() = songs.len();

        Ok(Some(path))
    }
}

// ============================================================
// RoomState — player + queue state for Host Mode
// ============================================================

/// Player status
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerStatus {
    Idle,
    Playing,
    Paused,
    Loading,
    Error,
}

/// Player state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: PlayerStatus,
    #[serde(rename = "currentSong")]
    pub current_song: Option<Song>,
    #[serde(rename = "currentTime")]
    pub current_time: f64,
    pub duration: f64,
    pub volume: u8,
    #[serde(rename = "isMuted")]
    pub is_muted: bool,
    #[serde(default = "default_auto_play_next", rename = "autoPlayNext")]
    pub auto_play_next: bool,
}

fn default_auto_play_next() -> bool {
    true
}

/// Connected client information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedClient {
    pub id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "connectedAt")]
    pub connected_at: i64,
}

/// Main room state (for Host Mode broadcasting)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomState {
    #[serde(rename = "roomId")]
    pub room_id: String,
    #[serde(rename = "hostPeerId")]
    pub host_peer_id: String,
    #[serde(rename = "connectedClients")]
    pub connected_clients: Vec<ConnectedClient>,
    pub player: PlayerState,
    pub queue: Vec<Song>,
    pub playlists: Vec<PlaylistCollection>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
}

impl RoomState {
    /// Create a new room state (playlists injected from PlaylistStore)
    pub fn new(room_id: String, host_peer_id: String, playlists: Vec<PlaylistCollection>) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            room_id,
            host_peer_id,
            connected_clients: Vec::new(),
            player: PlayerState {
                status: PlayerStatus::Idle,
                current_song: None,
                current_time: 0.0,
                duration: 0.0,
                volume: 80,
                is_muted: false,
                auto_play_next: true,
            },
            queue: Vec::new(),
            playlists,
            created_at: now,
            updated_at: now,
        }
    }

    /// Refresh playlists from PlaylistStore snapshot
    pub fn sync_playlists(&mut self, playlists: Vec<PlaylistCollection>) {
        self.playlists = playlists;
        self.touch();
    }

    /// Start a fresh host session while preserving persisted playlists.
    pub fn reset_for_new_session(&mut self, room_id: String, host_peer_id: String, playlists: Vec<PlaylistCollection>) {
        self.room_id = room_id;
        self.host_peer_id = host_peer_id;
        self.connected_clients.clear();
        self.player.current_song = None;
        self.player.status = PlayerStatus::Idle;
        self.player.current_time = 0.0;
        self.player.duration = 0.0;
        self.queue.clear();
        self.playlists = playlists;
        self.touch();
    }

    /// End the current host session without touching persisted playlists.
    pub fn shutdown_host_session(&mut self) {
        self.connected_clients.clear();
        self.player.current_song = None;
        self.player.status = PlayerStatus::Idle;
        self.player.current_time = 0.0;
        self.player.duration = 0.0;
        self.queue.clear();
        self.touch();
    }

    /// Add a song to the queue
    pub fn add_song(&mut self, song: Song) {
        if self.player.current_song.is_none() {
            self.player.current_song = Some(song);
            self.player.status = PlayerStatus::Loading;
            self.player.current_time = 0.0;
            self.player.duration = 0.0;
        } else {
            self.queue.push(song);
        }
        self.touch();
    }

    /// Remove a song from the queue by ID
    pub fn remove_song(&mut self, song_id: &str) -> bool {
        if self.player.current_song.as_ref().is_some_and(|song| song.id == song_id) {
            self.player.current_song = None;
            self.player.current_time = 0.0;
            self.player.duration = 0.0;
            self.player.status = PlayerStatus::Idle;
            self.touch();
            return true;
        }

        if let Some(pos) = self.queue.iter().position(|s| s.id == song_id) {
            self.queue.remove(pos);
            self.touch();
            true
        } else {
            false
        }
    }

    /// Reorder a song in the queue
    pub fn reorder_queue(&mut self, song_id: &str, new_index: usize) -> bool {
        if let Some(current_pos) = self.queue.iter().position(|s| s.id == song_id) {
            if new_index < self.queue.len() {
                let song = self.queue.remove(current_pos);
                self.queue.insert(new_index, song);
                self.touch();
                return true;
            }
        }
        false
    }

    /// Move a song up in the queue
    pub fn move_song_up(&mut self, song_id: &str) -> bool {
        if let Some(pos) = self.queue.iter().position(|s| s.id == song_id) {
            if pos > 0 {
                self.queue.swap(pos, pos - 1);
                self.touch();
                return true;
            }
        }
        false
    }

    /// Move a song down in the queue
    pub fn move_song_down(&mut self, song_id: &str) -> bool {
        if let Some(pos) = self.queue.iter().position(|s| s.id == song_id) {
            if pos < self.queue.len() - 1 {
                self.queue.swap(pos, pos + 1);
                self.touch();
                return true;
            }
        }
        false
    }

    /// Move a song to top of the queue
    pub fn move_song_to_top(&mut self, song_id: &str) -> bool {
        if let Some(pos) = self.queue.iter().position(|s| s.id == song_id) {
            if pos > 0 {
                let song = self.queue.remove(pos);
                self.queue.insert(0, song);
                self.touch();
                return true;
            }
        }
        false
    }

    /// Move a song to bottom of the queue
    pub fn move_song_to_bottom(&mut self, song_id: &str) -> bool {
        if let Some(pos) = self.queue.iter().position(|s| s.id == song_id) {
            if pos < self.queue.len() - 1 {
                let song = self.queue.remove(pos);
                self.queue.push(song);
                self.touch();
                return true;
            }
        }
        false
    }

    /// Update player state
    pub fn update_player(&mut self, status: Option<PlayerStatus>, current_time: Option<f64>, duration: Option<f64>) {
        if let Some(s) = status {
            self.player.status = s;
        }
        if let Some(t) = current_time {
            self.player.current_time = t;
        }
        if let Some(d) = duration {
            self.player.duration = d;
        }
        self.normalize_empty_player();
        self.touch();
    }

    /// Set volume
    pub fn set_volume(&mut self, volume: u8) {
        self.player.volume = volume.min(100);
        self.touch();
    }

    /// Toggle mute
    pub fn toggle_mute(&mut self) {
        self.player.is_muted = !self.player.is_muted;
        self.touch();
    }

    /// Skip to next song
    pub fn skip_song(&mut self, auto_play: bool) {
        if !self.queue.is_empty() {
            let next_song = self.queue.remove(0);
            self.player.current_song = Some(next_song);
            self.player.current_time = 0.0;
            self.player.duration = 0.0;
            self.player.status = if auto_play {
                PlayerStatus::Loading
            } else {
                PlayerStatus::Paused
            };
        } else {
            self.player.current_song = None;
            self.player.current_time = 0.0;
            self.player.duration = 0.0;
            self.player.status = PlayerStatus::Idle;
        }
        self.touch();
    }

    /// Play current song
    pub fn play(&mut self) {
        if self.player.current_song.is_some() {
            self.player.status = PlayerStatus::Playing;
            self.touch();
        } else if !self.queue.is_empty() {
            let next_song = self.queue.remove(0);
            self.player.current_song = Some(next_song);
            self.player.current_time = 0.0;
            self.player.duration = 0.0;
            self.player.status = PlayerStatus::Loading;
            self.touch();
        }
    }

    pub fn set_auto_play_next(&mut self, auto_play_next: bool) {
        self.player.auto_play_next = auto_play_next;
        self.touch();
    }

    pub fn stop_if_current_youtube_id(&mut self, youtube_id: &str) -> bool {
        if self.player.current_song.as_ref().is_some_and(|song| song.youtube_id == youtube_id) {
            self.player.current_song = None;
            self.player.current_time = 0.0;
            self.player.duration = 0.0;
            self.player.status = PlayerStatus::Idle;
            self.touch();
            true
        } else {
            false
        }
    }

    /// Pause playback
    pub fn pause(&mut self) {
        if self.player.current_song.is_some() {
            self.player.status = PlayerStatus::Paused;
        } else {
            self.player.status = PlayerStatus::Idle;
            self.player.current_time = 0.0;
            self.player.duration = 0.0;
        }
        self.touch();
    }

    /// Seek to a specific time
    pub fn seek(&mut self, time: f64) {
        self.player.current_time = time;
        self.touch();
    }

    /// Add a connected client
    #[allow(dead_code)]
    pub fn add_client(&mut self, client: ConnectedClient) {
        self.connected_clients.push(client);
        self.touch();
    }

    /// Remove a connected client by ID
    #[allow(dead_code)]
    pub fn remove_client(&mut self, client_id: &str) {
        self.connected_clients.retain(|c| c.id != client_id);
        self.touch();
    }

    /// Get a copy of the state with only public playlists (for broadcasting to remote clients)
    pub fn public_state(&self) -> RoomState {
        let mut state = self.clone();
        state.playlists.retain(|c| c.visibility == CollectionVisibility::Public);
        state
    }

    /// Update the timestamp
    fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().timestamp_millis();
    }

    fn normalize_empty_player(&mut self) {
        if self.player.current_song.is_none() {
            self.player.status = PlayerStatus::Idle;
            self.player.current_time = 0.0;
            self.player.duration = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> Song {
        Song {
            id: id.to_string(),
            youtube_id: "abc123".to_string(),
            title: "Song".to_string(),
            artist: "Artist".to_string(),
            duration: 180,
            original_title: None,
            original_artist: None,
            original_duration: None,
            thumbnail_url: "thumb.jpg".to_string(),
            added_by: "Singer".to_string(),
            added_at: 0,
            resolution_status: Some(ResolutionStatus::Resolved),
            source: Some(SongSource::Youtube {
                video_id: Some("abc123".to_string()),
                url: None,
            }),
        }
    }

    fn imported_spotify_collection(id: &str, playlist_id: &str, name: &str) -> PlaylistCollection {
        PlaylistCollection {
            id: id.to_string(),
            name: name.to_string(),
            visibility: CollectionVisibility::Personal,
            songs: vec![song("song")],
            description: None,
            source: Some(PlaylistSource::Spotify {
                playlist_id: playlist_id.to_string(),
                original_url: format!("https://open.spotify.com/playlist/{playlist_id}"),
                imported_at: 1,
            }),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn room() -> RoomState {
        RoomState::new("room".to_string(), "host".to_string(), Vec::new())
    }

    #[test]
    fn pause_without_a_current_song_keeps_player_idle() {
        let mut state = room();

        state.pause();

        assert!(matches!(state.player.status, PlayerStatus::Idle));
        assert!(state.player.current_song.is_none());
        assert_eq!(state.player.current_time, 0.0);
        assert_eq!(state.player.duration, 0.0);
    }

    #[test]
    fn skip_song_with_auto_play_on_loads_next_song() {
        let mut state = room();
        state.player.current_song = Some(song("current"));
        state.queue.push(song("next"));

        state.skip_song(true);

        assert_eq!(state.player.current_song.as_ref().map(|song| song.id.as_str()), Some("next"));
        assert!(matches!(state.player.status, PlayerStatus::Loading));
        assert_eq!(state.player.current_time, 0.0);
        assert_eq!(state.player.duration, 0.0);
    }

    #[test]
    fn skip_song_with_auto_play_off_cues_next_song_paused() {
        let mut state = room();
        state.player.current_song = Some(song("current"));
        state.queue.push(song("next"));

        state.skip_song(false);

        assert_eq!(state.player.current_song.as_ref().map(|song| song.id.as_str()), Some("next"));
        assert!(matches!(state.player.status, PlayerStatus::Paused));
        assert_eq!(state.player.current_time, 0.0);
        assert_eq!(state.player.duration, 0.0);
    }

    #[test]
    fn skip_song_with_empty_queue_clears_player_regardless_of_auto_play() {
        let mut state = room();
        state.player.current_song = Some(song("current"));
        state.player.status = PlayerStatus::Playing;
        state.player.current_time = 42.0;
        state.player.duration = 180.0;

        state.skip_song(true);

        assert!(state.player.current_song.is_none());
        assert!(matches!(state.player.status, PlayerStatus::Idle));
        assert_eq!(state.player.current_time, 0.0);
        assert_eq!(state.player.duration, 0.0);
    }

    #[test]
    fn playlist_store_detects_and_updates_spotify_reimport_without_duplicate() {
        let store = PlaylistStore::new();
        let first_id = store
            .upsert_imported_collection(imported_spotify_collection("local-1", "spotify123", "Fiesta"), false)
            .unwrap();
        assert_eq!(first_id, "local-1");
        assert_eq!(store.get_all().len(), 1);
        assert_eq!(
            store.upsert_imported_collection(imported_spotify_collection("local-2", "spotify123", "Fiesta 2"), false).unwrap_err(),
            "Esta playlist ya esta importada."
        );
        let updated_id = store
            .upsert_imported_collection(imported_spotify_collection("local-2", "spotify123", "Fiesta Updated"), true)
            .unwrap();
        let all = store.get_all();
        assert_eq!(updated_id, "local-1");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Fiesta Updated");
    }

    #[test]
    fn playlist_store_persists_imported_collection() {
        let dir = std::env::temp_dir().join(format!("festejar-playlist-store-test-{}", uuid::Uuid::new_v4()));
        let store = PlaylistStore::new();
        store.initialize(dir.clone());
        store
            .upsert_imported_collection(imported_spotify_collection("local-1", "spotify123", "Fiesta"), false)
            .unwrap();

        let restored = PlaylistStore::new();
        let loaded = restored.initialize(dir.clone());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Fiesta");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn playlist_store_replaces_song_in_place() {
        let store = PlaylistStore::new();
        let collection_id = store.create_collection("Songs".to_string(), CollectionVisibility::Personal);
        let original = Song {
            id: "song-1".to_string(),
            youtube_id: String::new(),
            title: "Unresolved".to_string(),
            artist: "Artist".to_string(),
            duration: 0,
            original_title: Some("Unresolved".to_string()),
            original_artist: Some("Artist".to_string()),
            original_duration: Some(0),
            thumbnail_url: String::new(),
            added_by: "Import".to_string(),
            added_at: 10,
            resolution_status: Some(ResolutionStatus::Unresolved),
            source: Some(SongSource::Local),
        };
        store.add_to_collection(&collection_id, original);

        let mut resolved = song("replacement");
        resolved.youtube_id = "resolvedid".to_string();
        assert!(store.replace_song_in_collection(&collection_id, "song-1", resolved));
        let all = store.get_all();
        assert_eq!(all[0].songs[0].id, "song-1");
        assert_eq!(all[0].songs[0].youtube_id, "resolvedid");
        assert_eq!(all[0].songs[0].added_at, 10);
    }

    #[test]
    fn playlist_store_moves_songs_between_collections() {
        let store = PlaylistStore::new();
        let source_id = store.create_collection("Source".to_string(), CollectionVisibility::Personal);
        let target_id = store.create_collection("Target".to_string(), CollectionVisibility::Personal);
        store.add_to_collection(&source_id, song("song-a"));
        store.add_to_collection(&source_id, song("song-b"));

        assert!(store.move_songs_to_collection(&source_id, &target_id, &["song-a".to_string()]));
        let all = store.get_all();
        let source = all.iter().find(|collection| collection.id == source_id).unwrap();
        let target = all.iter().find(|collection| collection.id == target_id).unwrap();
        assert_eq!(source.songs.iter().map(|song| song.id.as_str()).collect::<Vec<_>>(), vec!["song-b"]);
        assert_eq!(target.songs.iter().map(|song| song.id.as_str()).collect::<Vec<_>>(), vec!["song-a"]);
    }

    #[test]
    fn player_update_cannot_leave_playing_state_without_a_song() {
        let mut state = room();

        state.update_player(Some(PlayerStatus::Paused), Some(42.0), Some(180.0));

        assert!(matches!(state.player.status, PlayerStatus::Idle));
        assert_eq!(state.player.current_time, 0.0);
        assert_eq!(state.player.duration, 0.0);
    }

    #[test]
    fn play_from_queue_resets_stale_player_clock() {
        let mut state = room();
        state.player.current_time = 99.0;
        state.player.duration = 240.0;
        state.queue.push(song("queued"));

        state.play();

        assert!(matches!(state.player.status, PlayerStatus::Loading));
        assert_eq!(state.player.current_song.as_ref().map(|s| s.id.as_str()), Some("queued"));
        assert_eq!(state.player.current_time, 0.0);
        assert_eq!(state.player.duration, 0.0);
    }

    #[test]
    fn removing_the_current_song_stops_the_player_without_skipping() {
        let mut state = room();
        state.player.current_song = Some(song("current"));
        state.player.status = PlayerStatus::Playing;
        state.player.current_time = 77.0;
        state.player.duration = 180.0;
        state.queue.push(song("next"));

        assert!(state.remove_song("current"));

        assert!(matches!(state.player.status, PlayerStatus::Idle));
        assert!(state.player.current_song.is_none());
        assert_eq!(state.player.current_time, 0.0);
        assert_eq!(state.player.duration, 0.0);
        assert_eq!(state.queue.len(), 1);
        assert_eq!(state.queue[0].id, "next");
    }

    #[test]
    fn stop_if_current_youtube_id_stops_matching_loaded_video() {
        let mut state = room();
        let mut current = song("current");
        current.youtube_id = "same-video".to_string();
        state.player.current_song = Some(current);
        state.player.status = PlayerStatus::Playing;
        state.player.current_time = 33.0;
        state.player.duration = 180.0;

        assert!(state.stop_if_current_youtube_id("same-video"));

        assert!(matches!(state.player.status, PlayerStatus::Idle));
        assert!(state.player.current_song.is_none());
        assert_eq!(state.player.current_time, 0.0);
        assert_eq!(state.player.duration, 0.0);
    }

    #[test]
    fn shutting_down_host_session_clears_player_and_queue() {
        let mut state = room();
        state.add_song(song("current"));
        state.add_song(song("queued"));
        state.player.status = PlayerStatus::Playing;
        state.player.current_time = 42.0;
        state.player.duration = 180.0;

        state.shutdown_host_session();

        assert!(state.player.current_song.is_none());
        assert!(state.queue.is_empty());
        assert!(state.connected_clients.is_empty());
        assert!(matches!(state.player.status, PlayerStatus::Idle));
        assert_eq!(state.player.current_time, 0.0);
        assert_eq!(state.player.duration, 0.0);
    }
}

/// Thread-safe room state manager
pub struct RoomStateManager {
    state: Arc<RwLock<RoomState>>,
}

impl RoomStateManager {
    /// Create a new room state manager
    pub fn new(room_id: String, host_peer_id: String, playlists: Vec<PlaylistCollection>) -> Self {
        Self {
            state: Arc::new(RwLock::new(RoomState::new(room_id, host_peer_id, playlists))),
        }
    }

    /// Get a write lock on the state
    pub fn write(&self) -> parking_lot::RwLockWriteGuard<'_, RoomState> {
        self.state.write()
    }

    /// Clone the current state (full, including personal collections — for host UI)
    /// Clone only the player slice, for high-frequency progress broadcasts.
    pub fn clone_player(&self) -> PlayerState {
        self.state.read().player.clone()
    }

    pub fn clone_state(&self) -> RoomState {
        self.state.read().clone()
    }

    /// Clone a filtered state (public only — for broadcast to remote clients)
    pub fn clone_public_state(&self) -> RoomState {
        self.state.read().public_state()
    }
}
