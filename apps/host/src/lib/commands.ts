import { invoke } from '@tauri-apps/api/core';
import { RoomState, ClientCommand, PlaylistCollection } from '@karaokenatin/shared';

/**
 * Tauri command wrappers for Rust backend
 */

export async function createRoom(): Promise<{ roomId: string; joinToken: string }> {
    return await invoke('create_room');
}

export async function getRoomState(): Promise<RoomState> {
    return await invoke('get_room_state');
}

export async function getSessionHistory(): Promise<RoomState['queue']> {
    return await invoke('get_session_history');
}

export async function exportSessionHistory(): Promise<string | null> {
    return await invoke('export_session_history');
}

export interface AppSettingsInfo {
    productName: string;
    version: string;
    dataDir?: string | null;
    logDir?: string | null;
    playlistsPath?: string | null;
    sessionHistoryDir?: string | null;
}

export async function getAppSettingsInfo(): Promise<AppSettingsInfo> {
    return await invoke('get_app_settings_info');
}

export async function openDataFolder(): Promise<void> {
    return await invoke('open_data_folder');
}

export async function openSessionHistoryFolder(): Promise<void> {
    return await invoke('open_session_history_folder');
}

export async function openLogFolder(): Promise<void> {
    return await invoke('open_log_folder');
}

export async function openGithubRepository(): Promise<void> {
    return await invoke('open_github_repository');
}

export async function reportIssue(): Promise<void> {
    return await invoke('report_issue');
}

export async function processCommand(command: ClientCommand): Promise<void> {
    return await invoke('process_command', { command });
}

/**
 * Mirrors `update_player_state(status, current_time, duration)` in commands.rs.
 * Tauri maps these camelCase keys onto the snake_case Rust parameters; they are
 * passed flat, not nested under a `state` object.
 */
export async function updatePlayerState(state: {
    status?: string;
    currentTime?: number;
    duration?: number;
}): Promise<void> {
    return await invoke('update_player_state', {
        status: state.status,
        currentTime: state.currentTime,
        duration: state.duration,
    });
}

export async function openPlayerDisplay(): Promise<void> {
    return await invoke('open_player_display');
}

export async function closePlayerDisplay(): Promise<void> {
    return await invoke('close_player_display');
}

export async function shutdownHostSession(): Promise<void> {
    return await invoke('shutdown_host_session');
}

export async function setPlayerDisplayFullscreen(fullscreen: boolean): Promise<void> {
    return await invoke('set_player_display_fullscreen', { fullscreen });
}

export async function exportCollection(collectionId: string): Promise<string> {
    return await invoke('export_collection', { collectionId });
}

// ============================================================
// Standalone playlist commands (available in all modes)
// ============================================================

export async function getPlaylists(): Promise<PlaylistCollection[]> {
    return await invoke('get_playlists');
}

export async function playlistCreateCollection(
    name: string,
    visibility?: 'public' | 'personal',
): Promise<string> {
    return await invoke('playlist_create_collection', { name, visibility });
}

export async function playlistDeleteCollection(collectionId: string): Promise<void> {
    return await invoke('playlist_delete_collection', { collectionId });
}

export async function playlistRenameCollection(collectionId: string, name: string): Promise<void> {
    return await invoke('playlist_rename_collection', { collectionId, name });
}

export async function playlistSetVisibility(
    collectionId: string,
    visibility: 'public' | 'personal',
): Promise<void> {
    return await invoke('playlist_set_visibility', { collectionId, visibility });
}

export async function playlistAddSong(
    youtubeUrl: string,
    collectionId: string,
    addedBy?: string,
): Promise<void> {
    return await invoke('playlist_add_song', { youtubeUrl, collectionId, addedBy });
}

export async function playlistRemoveSong(collectionId: string, songId: string): Promise<void> {
    return await invoke('playlist_remove_song', { collectionId, songId });
}

export interface YouTubeSearchResult {
    id: string;
    url: string;
    title: string;
    channel: string;
    duration: string;
    thumbnail: string;
}

export async function playlistResolveSong(
    collectionId: string,
    songId: string,
    result: YouTubeSearchResult,
): Promise<void> {
    return await invoke('playlist_resolve_song', { collectionId, songId, result });
}

export async function playlistQueueCollection(collectionId: string, addedBy?: string): Promise<number> {
    return await invoke('playlist_queue_collection', { collectionId, addedBy });
}

export async function playlistMoveSongs(
    sourceCollectionId: string,
    targetCollectionId: string,
    songIds: string[],
): Promise<void> {
    return await invoke('playlist_move_songs', { sourceCollectionId, targetCollectionId, songIds });
}

export async function playlistImportCollection(data: string): Promise<string> {
    return await invoke('playlist_import_collection', { data });
}

export type ImportedPlaylistSource =
    | { type: 'local'; playlistId?: null; originalUrl?: null }
    | { type: 'spotify'; playlistId?: string | null; originalUrl?: string | null };

export type ImportedTrackSource =
    | { type: 'local'; playlistId?: null; trackId?: string | null; url?: string | null }
    | { type: 'spotify'; playlistId?: string | null; trackId?: string | null; url?: string | null };

export interface ImportedTrack {
    title: string;
    artists: string[];
    durationMs?: number | null;
    source: ImportedTrackSource;
    youtubeVideoId?: string | null;
    resolutionStatus: 'RESOLVED' | 'UNRESOLVED';
}

export interface ImportedPlaylist {
    name: string;
    description?: string | null;
    source: ImportedPlaylistSource;
    importedAt: number;
    tracks: ImportedTrack[];
}

export interface ImportPreview {
    playlist: ImportedPlaylist;
    validTrackCount: number;
    incompleteTrackCount: number;
    existingCollectionId?: string | null;
}

export async function previewSpotifyPlaylistImport(url: string): Promise<ImportPreview> {
    return await invoke('preview_spotify_playlist_import', { url });
}

export async function previewKaraokeJsonPlaylistImport(data: string): Promise<ImportPreview> {
    return await invoke('preview_karaoke_json_playlist_import', { data });
}

export async function confirmPlaylistImport(
    playlist: ImportedPlaylist,
    updateExisting: boolean,
): Promise<string> {
    return await invoke('confirm_playlist_import', { playlist, updateExisting });
}

export async function saveCollectionToFile(collectionId: string): Promise<void> {
    return await invoke('save_collection_to_file', { collectionId });
}

export async function loadCollectionFromFile(): Promise<string> {
    return await invoke('load_collection_from_file');
}

// ============================================================
// Lazy host server start
// ============================================================

export async function startHostServer(): Promise<number> {
    return await invoke('start_host_server');
}
