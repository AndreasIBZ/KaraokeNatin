use crate::room_state::{CollectionVisibility, PlaylistCollection, PlaylistSource, ResolutionStatus, Song, SongSource};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;
use uuid::Uuid;

const MAX_SPOTIFY_HTML_BYTES: usize = 2_000_000;
const SPOTIFY_TIMEOUT_SECONDS: u64 = 12;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ImportSourceType {
    Local,
    Spotify,
    Youtube,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedPlaylistSource {
    #[serde(rename = "type")]
    pub source_type: ImportSourceType,
    #[serde(default, rename = "playlistId")]
    pub playlist_id: Option<String>,
    #[serde(default, rename = "originalUrl")]
    pub original_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedTrackSource {
    #[serde(rename = "type")]
    pub source_type: ImportSourceType,
    #[serde(default, rename = "playlistId")]
    pub playlist_id: Option<String>,
    #[serde(default, rename = "trackId")]
    pub track_id: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedTrack {
    pub title: String,
    pub artists: Vec<String>,
    #[serde(default, rename = "durationMs")]
    pub duration_ms: Option<u32>,
    pub source: ImportedTrackSource,
    #[serde(default, rename = "youtubeVideoId")]
    pub youtube_video_id: Option<String>,
    #[serde(rename = "resolutionStatus")]
    pub resolution_status: ResolutionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedPlaylist {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub source: ImportedPlaylistSource,
    #[serde(rename = "importedAt")]
    pub imported_at: i64,
    pub tracks: Vec<ImportedTrack>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportPreview {
    pub playlist: ImportedPlaylist,
    #[serde(rename = "validTrackCount")]
    pub valid_track_count: usize,
    #[serde(rename = "incompleteTrackCount")]
    pub incomplete_track_count: usize,
    #[serde(rename = "existingCollectionId")]
    pub existing_collection_id: Option<String>,
}

pub trait PlaylistImporter {
    fn import(&self, input: &str) -> Result<ImportedPlaylist, String>;
}

pub struct KaraokeJsonImporter;

impl PlaylistImporter for KaraokeJsonImporter {
    fn import(&self, input: &str) -> Result<ImportedPlaylist, String> {
        parse_karaoke_json(input)
    }
}

pub struct SpotifyPlaylistImporter {
    html: String,
    playlist_id: String,
    original_url: String,
}

impl SpotifyPlaylistImporter {
    pub fn new(html: String, playlist_id: String, original_url: String) -> Self {
        Self { html, playlist_id, original_url }
    }
}

impl PlaylistImporter for SpotifyPlaylistImporter {
    fn import(&self, _input: &str) -> Result<ImportedPlaylist, String> {
        SpotifyEmbedParser::new(&self.playlist_id, &self.original_url).parse(&self.html)
    }
}

pub struct TextPlaylistImporter;

impl PlaylistImporter for TextPlaylistImporter {
    fn import(&self, input: &str) -> Result<ImportedPlaylist, String> {
        parse_text_playlist(input)
    }
}

pub struct SpotifyHttpClient {
    client: reqwest::Client,
}

impl SpotifyHttpClient {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(SPOTIFY_TIMEOUT_SECONDS))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent("FESTEJAR playlist importer")
            .build()
            .map_err(|e| format!("No se pudo preparar el importador Spotify: {}", e))?;
        Ok(Self { client })
    }

    pub async fn fetch_embed_html(&self, playlist_url: &str) -> Result<(String, String, String), String> {
        let playlist_id = parse_spotify_playlist_id(playlist_url)?;
        let embed_url = format!("https://open.spotify.com/embed/playlist/{}", playlist_id);
        let response = self.client
            .get(&embed_url)
            .send()
            .await
            .map_err(|e| {
                log::error!("Spotify import network error for {}: {}", embed_url, e);
                "No se pudo importar la playlist. Comprueba la conexion a Internet.".to_string()
            })?;

        let final_url = response.url().clone();
        validate_spotify_embed_url(&final_url)?;
        if !response.status().is_success() {
            log::error!("Spotify import failed with status {} for {}", response.status(), embed_url);
            return Err("No se puede acceder a esta playlist mediante el enlace compartido.".to_string());
        }

        let bytes = response.bytes().await.map_err(|e| {
            log::error!("Spotify import body read failed: {}", e);
            "No se pudo importar la playlist. Comprueba la conexion a Internet.".to_string()
        })?;
        if bytes.len() > MAX_SPOTIFY_HTML_BYTES {
            log::error!("Spotify import response too large: {} bytes", bytes.len());
            return Err("Spotify ha devuelto un formato que FESTEJAR no puede interpretar.".to_string());
        }
        let html = String::from_utf8(bytes.to_vec()).map_err(|e| {
            log::error!("Spotify import response was not UTF-8: {}", e);
            "Spotify ha devuelto un formato que FESTEJAR no puede interpretar.".to_string()
        })?;
        Ok((playlist_id, embed_url, html))
    }
}

pub fn parse_spotify_playlist_id(input: &str) -> Result<String, String> {
    let url = Url::parse(input.trim())
        .map_err(|_| "No parece una URL de playlist de Spotify.".to_string())?;
    if url.scheme() != "https" || url.host_str() != Some("open.spotify.com") {
        return Err("No parece una URL de playlist de Spotify.".to_string());
    }
    let mut segments = url.path_segments()
        .ok_or_else(|| "No parece una URL de playlist de Spotify.".to_string())?;
    match (segments.next(), segments.next(), segments.next()) {
        (Some("playlist"), Some(id), None) if is_valid_spotify_id(id) => Ok(id.to_string()),
        _ => Err("No parece una URL de playlist de Spotify.".to_string()),
    }
}

fn validate_spotify_embed_url(url: &Url) -> Result<(), String> {
    if url.scheme() == "https" && url.host_str() == Some("open.spotify.com") {
        return Ok(());
    }
    log::error!("Spotify import redirect left allowed domain: {}", url);
    Err("No se puede acceder a esta playlist mediante el enlace compartido.".to_string())
}

fn is_valid_spotify_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.chars().all(|c| c.is_ascii_alphanumeric())
}

pub struct SpotifyEmbedParser<'a> {
    playlist_id: &'a str,
    original_url: &'a str,
}

impl<'a> SpotifyEmbedParser<'a> {
    pub fn new(playlist_id: &'a str, original_url: &'a str) -> Self {
        Self { playlist_id, original_url }
    }

    pub fn parse(&self, html: &str) -> Result<ImportedPlaylist, String> {
        let mut tracks = Vec::new();
        let mut seen = HashSet::new();
        let mut name = extract_meta_content(html, "og:title")
            .or_else(|| extract_title(html))
            .unwrap_or_else(|| "Spotify Playlist".to_string());
        let mut description = extract_meta_content(html, "og:description");

        for json in extract_json_script_blocks(html) {
            match serde_json::from_str::<Value>(&json) {
                Ok(value) => {
                    if name == "Spotify Playlist" {
                        if let Some(found_name) = find_playlist_name(&value) {
                            name = found_name;
                        }
                    }
                    if description.is_none() {
                        description = find_string_key(&value, &["description"]);
                    }
                    collect_tracks_from_value(&value, self.playlist_id, &mut tracks, &mut seen);
                }
                Err(e) => log::warn!("Ignoring unparsable Spotify JSON script block: {}", e),
            }
        }

        if tracks.is_empty() {
            collect_tracks_from_html_fallback(html, self.playlist_id, &mut tracks, &mut seen);
        }
        if tracks.is_empty() {
            return Err("Spotify ha devuelto un formato que FESTEJAR no puede interpretar.".to_string());
        }

        Ok(ImportedPlaylist {
            name: clean_text(&name),
            description: description.map(|s| clean_text(&s)).filter(|s| !s.is_empty()),
            source: ImportedPlaylistSource {
                source_type: ImportSourceType::Spotify,
                playlist_id: Some(self.playlist_id.to_string()),
                original_url: Some(self.original_url.to_string()),
            },
            imported_at: chrono::Utc::now().timestamp_millis(),
            tracks,
        })
    }
}

fn parse_karaoke_json(input: &str) -> Result<ImportedPlaylist, String> {
    #[derive(Deserialize)]
    struct KaraokeFile {
        format: String,
        version: u32,
        #[serde(default)]
        name: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        tracks: Vec<KaraokeFileTrack>,
    }
    #[derive(Deserialize)]
    struct KaraokeFileTrack {
        title: String,
        artists: Vec<String>,
        #[serde(default, rename = "duration_ms")]
        duration_ms: Option<u32>,
        #[serde(default)]
        source: Option<KaraokeFileTrackSource>,
    }
    #[derive(Deserialize)]
    struct KaraokeFileTrackSource {
        #[serde(rename = "type")]
        source_type: String,
        #[serde(default, rename = "track_id")]
        track_id: Option<String>,
        #[serde(default)]
        url: Option<String>,
    }

    let file: KaraokeFile = serde_json::from_str(input)
        .map_err(|_| "El archivo no es una playlist compatible con FESTEJAR.".to_string())?;
    if file.format != "karaoke-playlist" {
        return Err("El archivo no es una playlist compatible con FESTEJAR.".to_string());
    }
    if file.version != 1 {
        return Err("Version de playlist no soportada.".to_string());
    }
    if file.name.trim().is_empty() {
        return Err("El archivo no es una playlist compatible con FESTEJAR.".to_string());
    }

    let tracks = file.tracks.into_iter()
        .filter(|track| !track.title.trim().is_empty() && !track.artists.is_empty())
        .map(|track| {
            let source = track.source.unwrap_or(KaraokeFileTrackSource {
                source_type: "local".to_string(),
                track_id: None,
                url: None,
            });
            let source_type = match source.source_type.as_str() {
                "spotify" => ImportSourceType::Spotify,
                "youtube" => ImportSourceType::Youtube,
                _ => ImportSourceType::Local,
            };
            let youtube_video_id = if source_type == ImportSourceType::Youtube {
                source.track_id.clone()
            } else {
                None
            };
            let resolution_status = if youtube_video_id.as_deref().map(is_valid_youtube_video_id).unwrap_or(false) {
                ResolutionStatus::Resolved
            } else {
                ResolutionStatus::Unresolved
            };

            ImportedTrack {
                title: clean_text(&track.title),
                artists: track.artists.into_iter().map(|a| clean_text(&a)).filter(|a| !a.is_empty()).collect(),
                duration_ms: track.duration_ms,
                source: ImportedTrackSource {
                    source_type,
                    playlist_id: None,
                    track_id: source.track_id,
                    url: source.url,
                },
                youtube_video_id,
                resolution_status,
            }
        })
        .filter(|track| !track.artists.is_empty())
        .collect();

    Ok(ImportedPlaylist {
        name: clean_text(&file.name),
        description: file.description.map(|s| clean_text(&s)),
        source: ImportedPlaylistSource { source_type: ImportSourceType::Local, playlist_id: None, original_url: None },
        imported_at: chrono::Utc::now().timestamp_millis(),
        tracks,
    })
}

fn parse_text_playlist(input: &str) -> Result<ImportedPlaylist, String> {
    if input.lines().any(|line| contains_youtube_playlist_url(line.trim())) {
        return Err("Los enlaces de playlist de YouTube todavia no se importan directamente. Pega titulos o enlaces de video individuales.".to_string());
    }

    let mut tracks = Vec::new();
    let mut seen_youtube_ids = HashSet::new();
    let mut seen_plain_text = HashSet::new();

    for line in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let parsed = parse_text_playlist_line(line);
        let Some(track) = parsed else { continue; };

        if let Some(video_id) = &track.youtube_video_id {
            if !seen_youtube_ids.insert(video_id.clone()) {
                continue;
            }
        } else {
            let key = normalize_text_import_key(line);
            if key.is_empty() || !seen_plain_text.insert(key) {
                continue;
            }
        }

        tracks.push(track);
    }

    if tracks.is_empty() {
        return Err("No se han encontrado canciones importables en el texto.".to_string());
    }

    Ok(ImportedPlaylist {
        name: "Imported Playlist".to_string(),
        description: Some("Imported from pasted text".to_string()),
        source: ImportedPlaylistSource {
            source_type: ImportSourceType::Local,
            playlist_id: None,
            original_url: None,
        },
        imported_at: chrono::Utc::now().timestamp_millis(),
        tracks,
    })
}

fn parse_text_playlist_line(line: &str) -> Option<ImportedTrack> {
    let (label, url) = split_text_playlist_label_and_url(line);
    let youtube_video_id = url.as_deref().and_then(parse_youtube_video_id);
    let (title, artists) = parse_plain_track_label(label.as_deref().unwrap_or(line), youtube_video_id.as_deref());
    if title.is_empty() || artists.is_empty() {
        return None;
    }

    let is_resolved = youtube_video_id.is_some();

    Some(ImportedTrack {
        title,
        artists,
        duration_ms: None,
        source: ImportedTrackSource {
            source_type: if is_resolved { ImportSourceType::Youtube } else { ImportSourceType::Local },
            playlist_id: None,
            track_id: youtube_video_id.clone(),
            url,
        },
        youtube_video_id,
        resolution_status: if is_resolved {
            ResolutionStatus::Resolved
        } else {
            ResolutionStatus::Unresolved
        },
    })
}

fn split_text_playlist_label_and_url(line: &str) -> (Option<String>, Option<String>) {
    if let Some((label, rest)) = line.split_once('|') {
        if let Some(url) = first_youtube_video_url(rest) {
            let clean_label = label.trim();
            return (
                if clean_label.is_empty() { None } else { Some(clean_label.to_string()) },
                Some(url),
            );
        }
    }

    if let Some(url) = first_youtube_video_url(line) {
        let label = line.replace(&url, "");
        let label = label.trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '|' | ':' | ','));
        return (
            if label.is_empty() { None } else { Some(label.to_string()) },
            Some(url),
        );
    }

    (Some(line.to_string()), None)
}

fn parse_plain_track_label(label: &str, youtube_video_id: Option<&str>) -> (String, Vec<String>) {
    let cleaned = clean_text(label);
    if cleaned.is_empty() {
        return youtube_video_id
            .map(|id| (format!("YouTube Video {}", id), vec!["YouTube".to_string()]))
            .unwrap_or_default();
    }

    for separator in [" - ", " -- "] {
        if let Some((artist, title)) = cleaned.split_once(separator) {
            let artist = clean_text(artist);
            let title = clean_text(title);
            if !artist.is_empty() && !title.is_empty() {
                return (title, vec![artist]);
            }
        }
    }

    (cleaned, vec!["Unknown Artist".to_string()])
}

fn first_youtube_video_url(input: &str) -> Option<String> {
    input.split_whitespace()
        .map(|part| part.trim_matches(|c: char| matches!(c, '"' | '\'' | ')' | '(' | '[' | ']' | '<' | '>' | ',' | ';')))
        .find(|part| parse_youtube_video_id(part).is_some())
        .map(str::to_string)
}

fn parse_youtube_video_id(input: &str) -> Option<String> {
    let url = Url::parse(input).ok()?;
    let host = url.host_str()?.trim_start_matches("www.").trim_start_matches("m.");
    let id = match host {
        "youtu.be" => url.path_segments()?.next().map(str::to_string),
        "youtube.com" | "music.youtube.com" => {
            let mut segments = url.path_segments()?;
            match segments.next() {
                Some("watch") => url.query_pairs()
                    .find(|(key, _)| key == "v")
                    .map(|(_, value)| value.into_owned()),
                Some("shorts") | Some("embed") => segments.next().map(str::to_string),
                _ => None,
            }
        }
        _ => None,
    }?;

    if is_valid_youtube_video_id(&id) {
        Some(id)
    } else {
        None
    }
}

fn contains_youtube_playlist_url(input: &str) -> bool {
    input.split_whitespace().any(|part| {
        let clean = part.trim_matches(|c: char| matches!(c, '"' | '\'' | ')' | '(' | '[' | ']' | '<' | '>' | ',' | ';'));
        if parse_youtube_video_id(clean).is_some() {
            return false;
        }
        let Ok(url) = Url::parse(clean) else { return false; };
        let Some(host) = url.host_str() else { return false; };
        let host = host.trim_start_matches("www.").trim_start_matches("m.");
        if host != "youtube.com" && host != "music.youtube.com" && host != "youtu.be" {
            return false;
        }
        if url.path_segments().and_then(|mut segments| segments.next()) == Some("playlist") {
            return true;
        }
        url.query_pairs().any(|(key, _)| key == "list")
    })
}

fn is_valid_youtube_video_id(value: &str) -> bool {
    value.len() == 11 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn normalize_text_import_key(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn imported_playlist_to_collection(imported: ImportedPlaylist, existing_id: Option<String>) -> PlaylistCollection {
    let now = chrono::Utc::now().timestamp_millis();
    let source = match &imported.source.source_type {
        ImportSourceType::Spotify => imported.source.playlist_id.as_ref().map(|playlist_id| PlaylistSource::Spotify {
            playlist_id: playlist_id.clone(),
            original_url: imported.source.original_url.clone().unwrap_or_default(),
            imported_at: imported.imported_at,
        }),
        ImportSourceType::Local | ImportSourceType::Youtube => Some(PlaylistSource::Local),
    };

    let songs = imported.tracks.into_iter().map(|track| {
        let youtube_id = track.youtube_video_id.unwrap_or_default();
        let artists = if track.artists.is_empty() { "Unknown Artist".to_string() } else { track.artists.join(", ") };
        Song {
            id: Uuid::new_v4().to_string(),
            youtube_id: youtube_id.clone(),
            title: track.title.clone(),
            artist: artists.clone(),
            duration: track.duration_ms.map(|ms| ms / 1000).unwrap_or(0),
            original_title: Some(track.title),
            original_artist: Some(artists),
            original_duration: track.duration_ms.map(|ms| ms / 1000),
            thumbnail_url: if youtube_id.is_empty() { String::new() } else { format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", youtube_id) },
            added_by: "Import".to_string(),
            added_at: now,
            resolution_status: Some(if youtube_id.is_empty() { ResolutionStatus::Unresolved } else { ResolutionStatus::Resolved }),
            source: Some(match track.source.source_type {
                ImportSourceType::Spotify => SongSource::Spotify {
                    playlist_id: track.source.playlist_id,
                    track_id: track.source.track_id,
                    url: track.source.url,
                },
                ImportSourceType::Youtube => SongSource::Youtube {
                    video_id: if youtube_id.is_empty() { track.source.track_id } else { Some(youtube_id) },
                    url: track.source.url,
                },
                ImportSourceType::Local => SongSource::Local,
            }),
        }
    }).collect();

    PlaylistCollection {
        id: existing_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        name: imported.name,
        visibility: CollectionVisibility::Personal,
        songs,
        description: imported.description,
        source,
        created_at: now,
        updated_at: now,
    }
}

pub fn spotify_playlist_id_from_import(imported: &ImportedPlaylist) -> Option<&str> {
    if imported.source.source_type == ImportSourceType::Spotify {
        imported.source.playlist_id.as_deref()
    } else {
        None
    }
}

fn extract_json_script_blocks(html: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while let Some(script_start) = html[cursor..].find("<script") {
        let start = cursor + script_start;
        let Some(open_end_rel) = html[start..].find('>') else { break; };
        let content_start = start + open_end_rel + 1;
        let attrs = &html[start..content_start];
        let Some(close_rel) = html[content_start..].find("</script>") else { break; };
        let content_end = content_start + close_rel;
        if attrs.contains("application/json") || attrs.contains("__NEXT_DATA__") {
            blocks.push(decode_html_entities(html[content_start..content_end].trim()));
        }
        cursor = content_end + "</script>".len();
    }
    blocks
}

fn collect_tracks_from_value(value: &Value, playlist_id: &str, out: &mut Vec<ImportedTrack>, seen: &mut HashSet<String>) {
    match value {
        Value::Array(items) => {
            if items.iter().any(looks_like_track_object) {
                for item in items {
                    if let Some(track) = track_from_value(item, playlist_id) {
                        let key = format!("{}|{}", track.source.track_id.clone().unwrap_or_default(), track.title);
                        if seen.insert(key) {
                            out.push(track);
                        }
                    }
                }
            } else {
                for item in items {
                    collect_tracks_from_value(item, playlist_id, out, seen);
                }
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_tracks_from_value(value, playlist_id, out, seen);
            }
        }
        _ => {}
    }
}

fn looks_like_track_object(value: &Value) -> bool {
    value.as_object().map(|map| {
        (map.contains_key("name") || map.contains_key("title")) &&
            (map.contains_key("artists") || map.contains_key("artist") || map.contains_key("subtitle"))
    }).unwrap_or(false)
}

fn track_from_value(value: &Value, playlist_id: &str) -> Option<ImportedTrack> {
    let map = value.as_object()?;
    let title = string_field(value, &["name", "title"])?;
    let artists = artists_from_value(value);
    if title.trim().is_empty() || artists.is_empty() {
        return None;
    }
    let track_id = string_field(value, &["id"])
        .or_else(|| string_field(value, &["trackId", "track_id"]))
        .or_else(|| string_field(value, &["uri"]).and_then(|uri| uri.strip_prefix("spotify:track:").map(str::to_string)));
    let source_url = string_field(value, &["url"])
        .or_else(|| track_id.as_ref().map(|id| format!("https://open.spotify.com/track/{}", id)));

    Some(ImportedTrack {
        title: clean_text(&title),
        artists,
        duration_ms: numeric_field(map.get("duration_ms")).or_else(|| numeric_field(map.get("durationMs"))),
        source: ImportedTrackSource {
            source_type: ImportSourceType::Spotify,
            playlist_id: Some(playlist_id.to_string()),
            track_id,
            url: source_url,
        },
        youtube_video_id: None,
        resolution_status: ResolutionStatus::Unresolved,
    })
}

fn artists_from_value(value: &Value) -> Vec<String> {
    if let Some(artists) = value.get("artists").and_then(Value::as_array) {
        return artists.iter().filter_map(|artist| {
            artist.as_str()
                .map(str::to_string)
                .or_else(|| string_field(artist, &["name", "title"]))
        }).map(|s| clean_text(&s)).filter(|s| !s.is_empty()).collect();
    }
    string_field(value, &["artist", "subtitle"])
        .map(|artist| artist.split(',').map(clean_text).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    for key in keys {
        if let Some(s) = object.get(*key).and_then(Value::as_str) {
            return Some(s.to_string());
        }
    }
    None
}

fn numeric_field(value: Option<&Value>) -> Option<u32> {
    value.and_then(|v| {
        v.as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .or_else(|| v.as_str().and_then(parse_duration_to_ms))
    })
}

fn find_playlist_name(value: &Value) -> Option<String> {
    if let Value::Object(map) = value {
        if map.get("type").and_then(Value::as_str) == Some("playlist") {
            if let Some(name) = string_field(value, &["name", "title"]) {
                return Some(name);
            }
        }
        for key in ["playlist", "entity", "data"] {
            if let Some(found) = map.get(key).and_then(find_playlist_name) {
                return Some(found);
            }
        }
        for child in map.values() {
            if let Some(found) = find_playlist_name(child) {
                return Some(found);
            }
        }
    } else if let Value::Array(items) = value {
        for item in items {
            if let Some(found) = find_playlist_name(item) {
                return Some(found);
            }
        }
    }
    None
}

fn find_string_key(value: &Value, keys: &[&str]) -> Option<String> {
    if let Value::Object(map) = value {
        for key in keys {
            if let Some(s) = map.get(*key).and_then(Value::as_str) {
                return Some(s.to_string());
            }
        }
        for child in map.values() {
            if let Some(found) = find_string_key(child, keys) {
                return Some(found);
            }
        }
    } else if let Value::Array(items) = value {
        for item in items {
            if let Some(found) = find_string_key(item, keys) {
                return Some(found);
            }
        }
    }
    None
}

fn collect_tracks_from_html_fallback(html: &str, playlist_id: &str, out: &mut Vec<ImportedTrack>, seen: &mut HashSet<String>) {
    for line in html.lines() {
        if !line.contains("data-track-title") {
            continue;
        }
        let title = extract_attr(line, "data-track-title").unwrap_or_default();
        let artist = extract_attr(line, "data-track-artist").unwrap_or_default();
        if title.trim().is_empty() || artist.trim().is_empty() {
            continue;
        }
        let key = format!("{}|{}", title, artist);
        if !seen.insert(key) {
            continue;
        }
        out.push(ImportedTrack {
            title: clean_text(&title),
            artists: artist.split(',').map(clean_text).filter(|s| !s.is_empty()).collect(),
            duration_ms: extract_attr(line, "data-duration-ms").and_then(|s| s.parse::<u32>().ok()),
            source: ImportedTrackSource {
                source_type: ImportSourceType::Spotify,
                playlist_id: Some(playlist_id.to_string()),
                track_id: extract_attr(line, "data-track-id"),
                url: extract_attr(line, "data-track-url"),
            },
            youtube_video_id: None,
            resolution_status: ResolutionStatus::Unresolved,
        });
    }
}

fn extract_meta_content(html: &str, property: &str) -> Option<String> {
    for needle in [
        format!("property=\"{}\"", property),
        format!("name=\"{}\"", property),
    ] {
        if let Some(pos) = html.find(&needle) {
            let start = html[..pos].rfind("<meta").unwrap_or(pos);
            let end = html[pos..].find('>').map(|i| pos + i)?;
            if let Some(content) = extract_attr(&html[start..end], "content") {
                return Some(content);
            }
        }
    }
    None
}

fn extract_title(html: &str) -> Option<String> {
    let start = html.find("<title>")? + "<title>".len();
    let end = html[start..].find("</title>").map(|i| start + i)?;
    Some(decode_html_entities(&html[start..end]).replace(" | Spotify", ""))
}

fn extract_attr(input: &str, attr: &str) -> Option<String> {
    let needle = format!("{}=\"", attr);
    let start = input.find(&needle)? + needle.len();
    let end = input[start..].find('"').map(|i| start + i)?;
    Some(decode_html_entities(&input[start..end]))
}

fn parse_duration_to_ms(input: &str) -> Option<u32> {
    let parts: Vec<_> = input.split(':').collect();
    if parts.len() == 2 {
        let minutes = parts[0].parse::<u32>().ok()?;
        let seconds = parts[1].parse::<u32>().ok()?;
        Some((minutes * 60 + seconds) * 1000)
    } else {
        None
    }
}

fn clean_text(input: &str) -> String {
    decode_html_entities(input)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect()
}

fn decode_html_entities(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

pub fn preview_from_imported(playlist: ImportedPlaylist, existing_collection_id: Option<String>) -> ImportPreview {
    let valid_track_count = playlist.tracks.iter()
        .filter(|t| !t.title.trim().is_empty() && !t.artists.is_empty())
        .count();
    ImportPreview {
        incomplete_track_count: playlist.tracks.len().saturating_sub(valid_track_count),
        valid_track_count,
        playlist,
        existing_collection_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOTIFY_FIXTURE: &str = r#"
        <html><head>
        <meta property="og:title" content="Fiesta" />
        <script id="__NEXT_DATA__" type="application/json">
        {
          "props": {
            "pageProps": {
              "state": {
                "data": {
                  "entity": {
                    "type": "playlist",
                    "name": "Fiesta",
                    "description": "Party songs",
                    "tracks": [
                      {"id":"track1","name":"Houdini","artists":[{"name":"Dua Lipa"}],"duration_ms":185000},
                      {"id":"track2","name":"Don't Stop Me Now","artists":[{"name":"Queen"}],"duration_ms":209000},
                      {"id":"track3","name":"Mr. Brightside","artists":[{"name":"The Killers"}],"duration_ms":222000}
                    ]
                  }
                }
              }
            }
          }
        }
        </script></head></html>
    "#;

    const SPOTIFY_INCOMPLETE_FIXTURE: &str = r#"
        <script type="application/json">
        {"playlist":{"type":"playlist","name":"Partial","tracks":[
          {"id":"ok","name":"Zombie","artists":["The Cranberries"],"durationMs":306000},
          {"id":"bad","name":"","artists":[]}
        ]}}
        </script>
    "#;

    #[test]
    fn spotify_playlist_url_valid() {
        assert_eq!(parse_spotify_playlist_id("https://open.spotify.com/playlist/ABC123").unwrap(), "ABC123");
        assert_eq!(parse_spotify_playlist_id("https://open.spotify.com/playlist/ABC123?si=xyz").unwrap(), "ABC123");
    }

    #[test]
    fn spotify_playlist_url_rejects_non_playlist_and_fake_domain() {
        assert!(parse_spotify_playlist_id("https://open.spotify.com/track/ABC123").is_err());
        assert!(parse_spotify_playlist_id("https://open-spotify.example.com/playlist/ABC123").is_err());
    }

    #[test]
    fn spotify_parser_extracts_playlist_and_ordered_tracks() {
        let parsed = SpotifyEmbedParser::new("playlist1", "https://open.spotify.com/playlist/playlist1")
            .parse(SPOTIFY_FIXTURE)
            .unwrap();
        assert_eq!(parsed.name, "Fiesta");
        assert_eq!(parsed.tracks.len(), 3);
        assert_eq!(parsed.tracks[0].title, "Houdini");
        assert_eq!(parsed.tracks[0].artists, vec!["Dua Lipa"]);
        assert_eq!(parsed.tracks[1].title, "Don't Stop Me Now");
        assert_eq!(parsed.tracks[2].duration_ms, Some(222000));
        assert_eq!(parsed.tracks[0].resolution_status, ResolutionStatus::Unresolved);
    }

    #[test]
    fn spotify_parser_ignores_incomplete_tracks() {
        let parsed = SpotifyEmbedParser::new("playlist1", "https://open.spotify.com/playlist/playlist1")
            .parse(SPOTIFY_INCOMPLETE_FIXTURE)
            .unwrap();
        assert_eq!(parsed.tracks.len(), 1);
        assert_eq!(parsed.tracks[0].title, "Zombie");
    }

    #[test]
    fn spotify_parser_returns_controlled_error_for_incompatible_html() {
        let err = SpotifyEmbedParser::new("playlist1", "https://open.spotify.com/playlist/playlist1")
            .parse("<html></html>")
            .unwrap_err();
        assert!(err.contains("Spotify"));
    }

    #[test]
    fn karaoke_json_imports_version_one() {
        let parsed = KaraokeJsonImporter.import(r#"{
          "format": "karaoke-playlist",
          "version": 1,
          "generator": "FESTEJAR",
          "name": "Fiesta",
          "tracks": [
            {"title":"Zombie","artists":["The Cranberries"],"duration_ms":306000,"source":{"type":"spotify","track_id":null,"url":null}}
          ]
        }"#).unwrap();
        assert_eq!(parsed.name, "Fiesta");
        assert_eq!(parsed.tracks.len(), 1);
        assert_eq!(parsed.tracks[0].resolution_status, ResolutionStatus::Unresolved);
    }

    #[test]
    fn karaoke_json_restores_youtube_source_as_resolved() {
        let parsed = KaraokeJsonImporter.import(r#"{
          "format": "karaoke-playlist",
          "version": 1,
          "name": "Videos",
          "tracks": [
            {"title":"Bohemian Rhapsody","artists":["Queen"],"duration_ms":354000,"source":{"type":"youtube","track_id":"fJ9rUzIMcZQ","url":"https://youtu.be/fJ9rUzIMcZQ"}}
          ]
        }"#).unwrap();
        assert_eq!(parsed.tracks[0].youtube_video_id.as_deref(), Some("fJ9rUzIMcZQ"));
        assert_eq!(parsed.tracks[0].source.source_type, ImportSourceType::Youtube);
        assert_eq!(parsed.tracks[0].resolution_status, ResolutionStatus::Resolved);
    }

    #[test]
    fn karaoke_json_rejects_invalid_json_format_and_future_version() {
        assert!(KaraokeJsonImporter.import("not json").is_err());
        assert!(KaraokeJsonImporter.import(r#"{"format":"wrong","version":1,"name":"x","tracks":[]}"#).is_err());
        assert!(KaraokeJsonImporter.import(r#"{"format":"karaoke-playlist","version":99,"name":"x","tracks":[]}"#).is_err());
    }

    #[test]
    fn text_importer_accepts_plain_lines_and_preserves_order() {
        let parsed = TextPlaylistImporter.import("Queen - Radio Ga Ga\r\nDua Lipa - Houdini\n  The Killers - Mr Brightside  ").unwrap();
        assert_eq!(parsed.tracks.len(), 3);
        assert_eq!(parsed.tracks[0].title, "Radio Ga Ga");
        assert_eq!(parsed.tracks[0].artists, vec!["Queen"]);
        assert_eq!(parsed.tracks[1].title, "Houdini");
        assert_eq!(parsed.tracks[2].resolution_status, ResolutionStatus::Unresolved);
    }

    #[test]
    fn text_importer_accepts_youtube_video_urls() {
        let parsed = TextPlaylistImporter.import(
            "Queen - We Are The Champions | https://www.youtube.com/watch?v=04854XqcfCY\r\nhttps://youtu.be/fJ9rUzIMcZQ\nhttps://youtube.com/shorts/9bZkp7q19f0",
        ).unwrap();
        assert_eq!(parsed.tracks.len(), 3);
        assert_eq!(parsed.tracks[0].title, "We Are The Champions");
        assert_eq!(parsed.tracks[0].youtube_video_id.as_deref(), Some("04854XqcfCY"));
        assert_eq!(parsed.tracks[0].source.source_type, ImportSourceType::Youtube);
        assert_eq!(parsed.tracks[0].resolution_status, ResolutionStatus::Resolved);
        assert_eq!(parsed.tracks[1].youtube_video_id.as_deref(), Some("fJ9rUzIMcZQ"));
        assert_eq!(parsed.tracks[2].youtube_video_id.as_deref(), Some("9bZkp7q19f0"));
    }

    #[test]
    fn text_importer_deduplicates_plain_text_and_youtube_ids() {
        let parsed = TextPlaylistImporter.import(
            "Queen - Radio Ga Ga\nqueen radio ga ga\nFirst | https://youtu.be/04854XqcfCY\nSecond | https://www.youtube.com/watch?v=04854XqcfCY",
        ).unwrap();
        assert_eq!(parsed.tracks.len(), 2);
        assert_eq!(parsed.tracks[0].title, "Radio Ga Ga");
        assert_eq!(parsed.tracks[1].youtube_video_id.as_deref(), Some("04854XqcfCY"));
    }

    #[test]
    fn text_importer_rejects_youtube_playlist_urls() {
        let err = TextPlaylistImporter.import("https://www.youtube.com/playlist?list=PL123").unwrap_err();
        assert!(err.contains("playlist de YouTube"));
    }
}
