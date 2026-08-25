import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ArrowLeft, Sun, Moon, Search, Plus, Music, Trash2, Pencil, Globe, Lock, Upload, ChevronDown } from 'lucide-react';
import { PlaylistCollection, Song } from '../hooks/useRoomState';
import { useSongSearch } from '../hooks/useSongSearch';
import PlaylistImportModal from './PlaylistImportModal';
import {
    getPlaylists,
    playlistCreateCollection,
    playlistDeleteCollection,
    playlistRenameCollection,
    playlistSetVisibility,
    playlistAddSong,
    playlistRemoveSong,
    playlistResolveSong,
    playlistQueueCollection,
    playlistMoveSongs,
    saveCollectionToFile,
    type YouTubeSearchResult,
} from '../lib/commands';
import { setHostInputFocused } from '../hooks/useRoomState';

interface LibraryProps {
    onBack: () => void;
}

const PLAYLIST_PAGE_SIZE = 20;

interface PlaylistSongRowProps {
    song: Song;
    index: number;
    selected: boolean;
    suggestions: YouTubeSearchResult[];
    resolving: boolean;
    selectedSuggestionId?: string;
    onSelect: (songId: string, value: boolean) => void;
    onRemove: (songId: string) => void;
    onResolve: (song: Song) => void;
    onSuggestionChange: (songId: string, videoId: string) => void;
    onApplySuggestion: (song: Song) => void;
}

function originalSongTitle(song: Song) {
    return song.originalTitle?.trim() || song.title;
}

function originalSongArtist(song: Song) {
    return song.originalArtist?.trim() || song.artist;
}

function originalSongDuration(song: Song) {
    return song.originalDuration ?? song.duration;
}

function formatSongDuration(seconds: number) {
    if (!Number.isFinite(seconds) || seconds <= 0) return '';
    const minutes = Math.floor(seconds / 60);
    const remainder = Math.floor(seconds % 60);
    return `${minutes}:${String(remainder).padStart(2, '0')}`;
}

function PlaylistSongRow({
    song,
    index,
    selected,
    suggestions,
    resolving,
    selectedSuggestionId,
    onSelect,
    onRemove,
    onResolve,
    onSuggestionChange,
    onApplySuggestion,
}: PlaylistSongRowProps) {
    const isUnresolved = !song.youtubeId || song.resolutionStatus === 'UNRESOLVED';
    const selectedSuggestion = suggestions.find((suggestion) => suggestion.id === selectedSuggestionId) || suggestions[0];
    const resolveLabel = isUnresolved ? 'Resolve' : 'Re-resolve';
    const duration = formatSongDuration(originalSongDuration(song));
    return (
        <div className="playlist-item">
            <input
                type="checkbox"
                className="playlist-select"
                checked={selected}
                onChange={(event) => onSelect(song.id, event.target.checked)}
                title="Select song"
            />
            <span className="playlist-number">{index + 1}</span>
            {song.thumbnailUrl ? (
                <img src={song.thumbnailUrl} alt="" loading="lazy" decoding="async" width={40} height={30} className="playlist-thumb" />
            ) : (
                <div className="playlist-thumb playlist-thumb-empty"><Music size={16} /></div>
            )}
            <div className="playlist-info">
                <div className="playlist-title">{originalSongTitle(song)}</div>
                <div className="playlist-meta">
                    {originalSongArtist(song)}
                    {duration ? ` • ${duration}` : ''}
                    {isUnresolved ? ' • unresolved' : ' • resolved'}
                </div>
            </div>
            {suggestions.length > 0 ? (
                <div className="playlist-resolve-picker">
                    <select
                        value={selectedSuggestion?.id || ''}
                        onChange={(event) => onSuggestionChange(song.id, event.target.value)}
                        title="Choose YouTube match"
                    >
                        {suggestions.map((suggestion) => (
                            <option value={suggestion.id} key={suggestion.id}>
                                {suggestion.title} - {suggestion.channel}
                            </option>
                        ))}
                    </select>
                    <button
                        className="playlist-resolve-btn"
                        onClick={() => onApplySuggestion(song)}
                        disabled={!selectedSuggestion}
                        title="Replace this playlist entry"
                    >
                        Apply
                    </button>
                </div>
            ) : (
                <button
                    className="playlist-resolve-btn"
                    onClick={() => onResolve(song)}
                    disabled={resolving}
                    title={isUnresolved ? 'Find YouTube matches' : 'Find a better YouTube match'}
                >
                    {resolving ? 'Resolving...' : resolveLabel}
                </button>
            )}
            <button
                className="playlist-remove-btn"
                onClick={() => onRemove(song.id)}
                title="Remove from library"
            >
                <Trash2 size={14} />
            </button>
        </div>
    );
}

function normalizeMatchText(value: string) {
    return value.toLowerCase().normalize('NFD').replace(/[\u0300-\u036f]/g, '').replace(/[^a-z0-9]+/g, ' ').trim();
}

function matchScore(song: Song, result: YouTubeSearchResult) {
    const wanted = normalizeMatchText(`${originalSongTitle(song)} ${originalSongArtist(song)}`);
    const candidate = normalizeMatchText(`${result.title} ${result.channel}`);
    if (!wanted || !candidate) return 0;
    const words = wanted.split(' ').filter((word) => word.length > 2);
    const hits = words.filter((word) => candidate.includes(word)).length;
    return hits / Math.max(words.length, 1);
}

/**
 * Standalone Library Management Page
 * Allows managing personal song collections with full organization features
 * Can search for songs even without an active session
 */
export default function Library({ onBack }: LibraryProps) {
    const [theme, setTheme] = useState<'dark' | 'light'>('dark');
    const [collections, setCollections] = useState<PlaylistCollection[]>([]);
    const [activeCollectionId, setActiveCollectionId] = useState<string | null>(null);
    const [searchQuery, setSearchQuery] = useState('');
    const songSearch = useSongSearch(10);
    const {
        results: searchResults,
        loading: searching,
        karaokeOnly,
        setKaraokeOnly,
        search,
    } = songSearch;
    const [showImportModal, setShowImportModal] = useState(false);
    const [playlistPage, setPlaylistPage] = useState(0);
    const [selectedSongIds, setSelectedSongIds] = useState<Set<string>>(new Set());
    const [resolutionSuggestions, setResolutionSuggestions] = useState<Record<string, YouTubeSearchResult[]>>({});
    const [selectedSuggestions, setSelectedSuggestions] = useState<Record<string, string>>({});
    const [resolvingSongIds, setResolvingSongIds] = useState<Set<string>>(new Set());
    const [bulkMoveTargetId, setBulkMoveTargetId] = useState('');
    const [queueingCollection, setQueueingCollection] = useState(false);

    // Collection management
    const [newCollectionName, setNewCollectionName] = useState('');
    const [showNewCollection, setShowNewCollection] = useState(false);
    const [renamingCollectionId, setRenamingCollectionId] = useState<string | null>(null);
    const [renameValue, setRenameValue] = useState('');

    // Dropdown state for search results
    const [pickerOpenFor, setPickerOpenFor] = useState<string | null>(null);
    const [showNewCollectionInPicker, setShowNewCollectionInPicker] = useState(false);
    const [newCollectionNameInPicker, setNewCollectionNameInPicker] = useState('');

    // Loading states
    const [addingToLibrary, setAddingToLibrary] = useState<Set<string>>(new Set());
    const [addedToLibrary, setAddedToLibrary] = useState<Set<string>>(new Set());

    // Load collections on mount
    useEffect(() => {
        loadCollections();
    }, []);

    useEffect(() => {
        setPlaylistPage(0);
        setSelectedSongIds(new Set());
        setResolutionSuggestions({});
        setSelectedSuggestions({});
        setBulkMoveTargetId('');
    }, [activeCollectionId]);

    const loadCollections = async () => {
        try {
            const playlists = await getPlaylists();
            setCollections(playlists);
            if (!activeCollectionId && playlists.length > 0) {
                setActiveCollectionId(playlists[0].id);
            }
        } catch (error) {
            console.error('[Library] Failed to load collections:', error);
        }
    };

    const handleSearch = async (e: React.FormEvent) => {
        e.preventDefault();
        setAddedToLibrary(new Set());
        await search(searchQuery.trim());
    };

    const searchResolutionCandidates = useCallback(async (song: Song) => {
        const query = [originalSongTitle(song), originalSongArtist(song)].filter(Boolean).join(' ');
        if (!query.trim()) return;
        setResolvingSongIds((prev) => new Set(prev).add(song.id));
        try {
            const results = await invoke<YouTubeSearchResult[]>('search_youtube', {
                query,
                limit: 5,
                karaokeOnly,
            });
            const sorted = [...results].sort((a, b) => matchScore(song, b) - matchScore(song, a));
            setResolutionSuggestions((prev) => ({ ...prev, [song.id]: sorted }));
            if (sorted[0]) {
                setSelectedSuggestions((prev) => ({ ...prev, [song.id]: sorted[0].id }));
            }
        } catch (error) {
            console.error('[Library] Resolve search failed:', error);
        } finally {
            setResolvingSongIds((prev) => {
                const next = new Set(prev);
                next.delete(song.id);
                return next;
            });
        }
    }, [karaokeOnly]);

    const handleResolveImportedSong = useCallback((song: Song) => {
        void searchResolutionCandidates(song);
    }, [searchResolutionCandidates]);

    const applyResolutionSuggestion = useCallback(async (song: Song) => {
        if (!activeCollectionId) return;
        const suggestions = resolutionSuggestions[song.id] || [];
        const selectedId = selectedSuggestions[song.id] || suggestions[0]?.id;
        const result = suggestions.find((suggestion) => suggestion.id === selectedId);
        if (!result) return;

        setResolvingSongIds((prev) => new Set(prev).add(song.id));
        try {
            await playlistResolveSong(activeCollectionId, song.id, result);
            setResolutionSuggestions((prev) => {
                const next = { ...prev };
                delete next[song.id];
                return next;
            });
            setSelectedSuggestions((prev) => {
                const next = { ...prev };
                delete next[song.id];
                return next;
            });
            setSelectedSongIds((prev) => {
                const next = new Set(prev);
                next.delete(song.id);
                return next;
            });
            await loadCollections();
        } catch (error) {
            console.error('[Library] Apply resolution failed:', error);
            alert(typeof error === 'string' ? error : 'Failed to resolve song');
        } finally {
            setResolvingSongIds((prev) => {
                const next = new Set(prev);
                next.delete(song.id);
                return next;
            });
        }
    }, [activeCollectionId, resolutionSuggestions, selectedSuggestions]);

    const handleCreateCollection = async () => {
        if (!newCollectionName.trim()) return;
        try {
            await playlistCreateCollection(newCollectionName.trim(), 'personal');
            setNewCollectionName('');
            setShowNewCollection(false);
            await loadCollections();
        } catch (error) {
            console.error('[Library] Create collection failed:', error);
        }
    };

    const handleCreateCollectionInPicker = async (thenAddUrl?: string) => {
        if (!newCollectionNameInPicker.trim()) return;
        try {
            const newId = await playlistCreateCollection(newCollectionNameInPicker.trim(), 'personal');
            setNewCollectionNameInPicker('');
            setShowNewCollectionInPicker(false);
            await loadCollections();

            // If we were adding a song, add it to the new collection
            if (thenAddUrl) {
                await handlePickCollection(thenAddUrl, newId);
            }
            setPickerOpenFor(null);
        } catch (error) {
            console.error('[Library] Create collection in picker failed:', error);
        }
    };

    const handleDeleteCollection = async (collectionId: string) => {
        const collection = collections.find(c => c.id === collectionId);
        if (!collection) return;

        if (!confirm(`Delete "${collection.name}"?`)) return;

        try {
            await playlistDeleteCollection(collectionId);
            if (activeCollectionId === collectionId) {
                setActiveCollectionId(collections.find(c => c.id !== collectionId)?.id ?? null);
            }
            await loadCollections();
        } catch (error) {
            console.error('[Library] Delete collection failed:', error);
        }
    };

    const handleRenameCollection = async (collectionId: string) => {
        if (!renameValue.trim()) {
            setRenamingCollectionId(null);
            return;
        }
        try {
            await playlistRenameCollection(collectionId, renameValue.trim());
            await loadCollections();
        } catch (error) {
            console.error('[Library] Rename failed:', error);
        } finally {
            setRenamingCollectionId(null);
        }
    };

    const handleToggleVisibility = async (collection: PlaylistCollection) => {
        try {
            const newVis = collection.visibility === 'public' ? 'personal' : 'public';
            await playlistSetVisibility(collection.id, newVis);
            await loadCollections();
        } catch (error) {
            console.error('[Library] Toggle visibility failed:', error);
        }
    };

    const handlePickCollection = async (url: string, collectionId: string) => {
        setPickerOpenFor(null);
        setAddingToLibrary(prev => new Set(prev).add(url));
        try {
            await playlistAddSong(url, collectionId, 'Library');
            setAddedToLibrary(prev => new Set(prev).add(url));
            await loadCollections();
        } catch (error) {
            console.error('[Library] Add to library failed:', error);
        } finally {
            setAddingToLibrary(prev => { const s = new Set(prev); s.delete(url); return s; });
        }
    };

    const handleRemoveFromLibrary = async (collectionId: string, songId: string) => {
        try {
            await playlistRemoveSong(collectionId, songId);
            await loadCollections();
        } catch (error) {
            console.error('[Library] Remove from library failed:', error);
        }
    };

    // Stable per-collection callback so react-window's rowProps identity
    // only changes when the active collection actually changes.
    const handleRemoveFromActiveCollection = useCallback((songId: string) => {
        if (activeCollectionId) {
            handleRemoveFromLibrary(activeCollectionId, songId);
        }
    }, [activeCollectionId]);



    const handleSaveToFile = async (collectionId: string) => {
        try {
            await saveCollectionToFile(collectionId);
        } catch (error) {
            console.error('[Library] Save to file failed:', error);
            if (typeof error === 'string' && error.includes('cancelled')) return;
            alert('Failed to save file');
        }
    };

    const handleLoadFromFile = async () => {
        setShowImportModal(true);
    };

    const toggleTheme = () => {
        const newTheme = theme === 'dark' ? 'light' : 'dark';
        setTheme(newTheme);
        document.documentElement.classList.toggle('light', newTheme === 'light');
    };

    const activeCollection = collections.find(c => c.id === activeCollectionId);
    const totalSongs = collections.reduce((sum, c) => sum + c.songs.length, 0);
    const pageCount = activeCollection ? Math.max(1, Math.ceil(activeCollection.songs.length / PLAYLIST_PAGE_SIZE)) : 1;
    const safePage = Math.min(playlistPage, pageCount - 1);
    const visibleSongs = activeCollection?.songs.slice(
        safePage * PLAYLIST_PAGE_SIZE,
        safePage * PLAYLIST_PAGE_SIZE + PLAYLIST_PAGE_SIZE,
    ) || [];
    const visibleSongIds = visibleSongs.map((song) => song.id);
    const selectedVisibleCount = visibleSongIds.filter((id) => selectedSongIds.has(id)).length;
    const selectedSongs = activeCollection?.songs.filter((song) => selectedSongIds.has(song.id)) || [];
    const selectedResolvableSongs = selectedSongs;
    const resolvedSongCount = activeCollection?.songs.filter((song) => song.youtubeId && song.resolutionStatus !== 'UNRESOLVED').length || 0;

    const handleSelectSong = (songId: string, value: boolean) => {
        setSelectedSongIds((prev) => {
            const next = new Set(prev);
            if (value) {
                next.add(songId);
            } else {
                next.delete(songId);
            }
            return next;
        });
    };

    const handleSelectVisible = (value: boolean) => {
        setSelectedSongIds((prev) => {
            const next = new Set(prev);
            visibleSongIds.forEach((id) => {
                if (value) {
                    next.add(id);
                } else {
                    next.delete(id);
                }
            });
            return next;
        });
    };

    const handleBulkResolve = async () => {
        for (const song of selectedResolvableSongs) {
            if (!resolutionSuggestions[song.id]?.length) {
                await searchResolutionCandidates(song);
            }
        }
    };

    const handleApplySelectedResolutions = async () => {
        for (const song of selectedResolvableSongs) {
            if (resolutionSuggestions[song.id]?.length) {
                await applyResolutionSuggestion(song);
            }
        }
    };

    const handleBulkDelete = async () => {
        if (!activeCollectionId || selectedSongIds.size === 0) return;
        if (!confirm(`Delete ${selectedSongIds.size} selected songs?`)) return;
        for (const songId of Array.from(selectedSongIds)) {
            await playlistRemoveSong(activeCollectionId, songId);
        }
        setSelectedSongIds(new Set());
        await loadCollections();
    };

    const handleBulkMove = async () => {
        if (!activeCollectionId || !bulkMoveTargetId || selectedSongIds.size === 0) return;
        try {
            await playlistMoveSongs(activeCollectionId, bulkMoveTargetId, Array.from(selectedSongIds));
            setSelectedSongIds(new Set());
            setBulkMoveTargetId('');
            await loadCollections();
        } catch (error) {
            console.error('[Library] Move songs failed:', error);
            alert(typeof error === 'string' ? error : 'Failed to move songs');
        }
    };

    const handleQueueActiveCollection = async () => {
        if (!activeCollectionId || resolvedSongCount === 0) return;
        setQueueingCollection(true);
        try {
            const count = await playlistQueueCollection(activeCollectionId, 'Host');
            alert(`${count} canciones encoladas.`);
        } catch (error) {
            console.error('[Library] Queue playlist failed:', error);
            alert(typeof error === 'string' ? error : 'No se pudo encolar la playlist');
        } finally {
            setQueueingCollection(false);
        }
    };

    return (
        <div className="library-page">
            {/* Header */}
            <div className="library-header">
                <button className="btn-icon" onClick={onBack} title="Back to mode selection">
                    <ArrowLeft size={18} />
                </button>
                <h1 className="library-title">My Library</h1>
                <button className="btn-icon" onClick={toggleTheme}>
                    {theme === 'dark' ? <Sun size={18} /> : <Moon size={18} />}
                </button>
            </div>

            {/* Search Bar */}
            <div className="library-search">
                <form onSubmit={handleSearch} className="search-row">
                    <input
                        type="text"
                        className="search-input"
                        placeholder="Search for songs..."
                        value={searchQuery}
                        onChange={(e) => setSearchQuery(e.target.value)}
                        onFocus={() => setHostInputFocused(true)}
                        onBlur={() => setHostInputFocused(false)}
                    />
                    <button type="submit" className="btn-icon" title="Search">
                        <Search size={18} />
                    </button>
                </form>
            </div>

            <div className="library-content">
                {showImportModal && (
                    <PlaylistImportModal
                        onClose={() => setShowImportModal(false)}
                        onImported={async (collectionId) => {
                            await loadCollections();
                            setActiveCollectionId(collectionId);
                        }}
                    />
                )}

                {/* Search Results */}
                {searching && (
                    <div className="search-loading">
                        <div className="spinner"></div>
                        <p>Searching...</p>
                    </div>
                )}

                {!searching && searchResults.length > 0 && (
                    <div className="library-section">
                        <div className="section-label">Search Results</div>
                        <div className="search-results">
                            {searchResults.map((result) => {
                                const isLoading = addingToLibrary.has(result.url);
                                const isAdded = addedToLibrary.has(result.url);

                                return (
                                    <div key={result.url} className="search-result-item">
                                        <img src={result.thumbnail} alt="" loading="lazy" decoding="async" width={72} height={54} className="search-result-thumb" />
                                        <div className="search-result-info">
                                            <div className="search-result-title">{result.title}</div>
                                            <div className="search-result-meta">
                                                {result.channel} • {result.duration}
                                            </div>
                                            <div className="search-result-actions">
                                                <div className="playlist-picker-wrapper" style={{ position: 'relative' }}>
                                                    <button
                                                        className={`btn-sm ${isAdded ? 'btn-success' : 'btn-primary'}`}
                                                        onClick={() => setPickerOpenFor(pickerOpenFor === result.url ? null : result.url)}
                                                        disabled={isLoading}
                                                    >
                                                        {isLoading ? (
                                                            <>Saving...</>
                                                        ) : isAdded ? (
                                                            <>✓ Saved</>
                                                        ) : (
                                                            <>
                                                                <Plus size={14} /> Add to Library <ChevronDown size={12} />
                                                            </>
                                                        )}
                                                    </button>

                                                    {pickerOpenFor === result.url && (
                                                        <div className="collection-picker">
                                                            <div className="collection-picker-title">Add to Collection</div>
                                                            {collections.map(col => (
                                                                <button
                                                                    key={col.id}
                                                                    className="collection-picker-item"
                                                                    onClick={() => handlePickCollection(result.url, col.id)}
                                                                >
                                                                    <span className={`visibility-dot ${col.visibility}`}></span>
                                                                    {col.name}
                                                                    <span className="collection-picker-count">{col.songs.length}</span>
                                                                </button>
                                                            ))}
                                                            <div className="collection-picker-divider"></div>
                                                            {showNewCollectionInPicker ? (
                                                                <div className="collection-picker-new">
                                                                    <input
                                                                        type="text"
                                                                        placeholder="Collection name..."
                                                                        value={newCollectionNameInPicker}
                                                                        onChange={(e) => setNewCollectionNameInPicker(e.target.value)}
                                                                        onFocus={() => setHostInputFocused(true)}
                                                                        onBlur={() => setHostInputFocused(false)}
                                                                        onKeyDown={(e) => {
                                                                            if (e.key === 'Enter') {
                                                                                e.preventDefault();
                                                                                handleCreateCollectionInPicker(result.url);
                                                                            }
                                                                            e.stopPropagation();
                                                                        }}
                                                                        autoFocus
                                                                        className="collection-picker-input"
                                                                    />
                                                                    <button
                                                                        className="btn-sm btn-primary"
                                                                        onClick={() => handleCreateCollectionInPicker(result.url)}
                                                                    >
                                                                        Create
                                                                    </button>
                                                                </div>
                                                            ) : (
                                                                <button
                                                                    className="collection-picker-item collection-picker-create"
                                                                    onClick={() => setShowNewCollectionInPicker(true)}
                                                                >
                                                                    <Plus size={14} /> New Collection
                                                                </button>
                                                            )}
                                                        </div>
                                                    )}
                                                </div>
                                            </div>
                                        </div>
                                    </div>
                                );
                            })}
                        </div>
                    </div>
                )}

                {/* Collections Management */}
                <div className="library-section">
                    <div className="section-label" style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                        <span><Music size={16} style={{ display: 'inline', verticalAlign: '-2px' }} /> Collections ({totalSongs} songs)</span>
                        <div style={{ display: 'flex', gap: '4px' }}>
                            <button
                                className="btn-sm btn-secondary"
                                onClick={handleLoadFromFile}
                                title="Import from file"
                            >
                                <Upload size={14} style={{ display: 'inline', verticalAlign: '-2px', marginRight: '4px' }} /> Import
                            </button>
                        </div>
                    </div>

                    {/* Collection Tabs */}
                    <div className="collection-tabs">
                        {collections.map(col => (
                            <button
                                key={col.id}
                                className={`collection-tab ${activeCollectionId === col.id ? 'active' : ''}`}
                                onClick={() => setActiveCollectionId(col.id)}
                                title={`${col.name} (${col.visibility})`}
                            >
                                <span className={`visibility-dot ${col.visibility}`}></span>
                                {renamingCollectionId === col.id ? (
                                    <input
                                        className="collection-rename-input"
                                        value={renameValue}
                                        onChange={(e) => setRenameValue(e.target.value)}
                                        onFocus={() => setHostInputFocused(true)}
                                        onBlur={() => { setHostInputFocused(false); handleRenameCollection(col.id); }}
                                        onKeyDown={(e) => {
                                            if (e.key === 'Enter') handleRenameCollection(col.id);
                                            if (e.key === 'Escape') setRenamingCollectionId(null);
                                            e.stopPropagation();
                                        }}
                                        autoFocus
                                        onClick={(e) => e.stopPropagation()}
                                    />
                                ) : (
                                    <span>{col.name} ({col.songs.length})</span>
                                )}
                            </button>
                        ))}
                        <button
                            className="collection-tab collection-tab-add"
                            onClick={() => setShowNewCollection(true)}
                            title="Create new collection"
                        >
                            <Plus size={14} />
                        </button>
                    </div>

                    {/* New Collection Form */}
                    {showNewCollection && (
                        <div className="library-new-collection">
                            <input
                                type="text"
                                placeholder="Collection name..."
                                value={newCollectionName}
                                onChange={(e) => setNewCollectionName(e.target.value)}
                                onFocus={() => setHostInputFocused(true)}
                                onBlur={() => setHostInputFocused(false)}
                                onKeyDown={(e) => {
                                    if (e.key === 'Enter') {
                                        e.preventDefault();
                                        handleCreateCollection();
                                    }
                                    if (e.key === 'Escape') {
                                        setShowNewCollection(false);
                                        setNewCollectionName('');
                                    }
                                }}
                                autoFocus
                                className="search-input"
                                style={{ marginBottom: '8px' }}
                            />
                            <div style={{ display: 'flex', gap: '8px' }}>
                                <button className="btn-primary" onClick={handleCreateCollection}>
                                    Create
                                </button>
                                <button
                                    className="btn-secondary"
                                    onClick={() => {
                                        setShowNewCollection(false);
                                        setNewCollectionName('');
                                    }}
                                >
                                    Cancel
                                </button>
                            </div>
                        </div>
                    )}

                    {/* Collection Actions */}
                    {activeCollection && (
                        <div className="collection-actions-bar">
                            <label className="playlist-resolve-mode">
                                <input
                                    type="checkbox"
                                    checked={karaokeOnly}
                                    onChange={(event) => setKaraokeOnly(event.target.checked)}
                                />
                                <span>Karaoke</span>
                            </label>
                            <button
                                className="btn-sm btn-secondary"
                                onClick={handleQueueActiveCollection}
                                disabled={queueingCollection || resolvedSongCount === 0}
                                title="Queue resolved songs in playlist order"
                            >
                                <Plus size={13} /> {queueingCollection ? 'Queueing...' : `Queue ${resolvedSongCount}`}
                            </button>
                            <button
                                className="btn-sm btn-secondary"
                                onClick={() => handleToggleVisibility(activeCollection)}
                                title={activeCollection.visibility === 'public' ? 'Make personal' : 'Make public'}
                            >
                                {activeCollection.visibility === 'public' ? (
                                    <><Globe size={13} /> Public</>
                                ) : (
                                    <><Lock size={13} /> Personal</>
                                )}
                            </button>
                            <button
                                className="btn-sm btn-secondary"
                                onClick={() => {
                                    setRenamingCollectionId(activeCollection.id);
                                    setRenameValue(activeCollection.name);
                                }}
                                title="Rename"
                            >
                                <Pencil size={13} />
                            </button>
                            <button
                                className="btn-sm btn-secondary"
                                onClick={() => handleSaveToFile(activeCollection.id)}
                                title="Export to file"
                            >
                                <Upload size={13} /> Export
                            </button>
                            {collections.length > 1 && (
                                <button
                                    className="btn-sm btn-danger-text"
                                    onClick={() => handleDeleteCollection(activeCollection.id)}
                                    title="Delete collection"
                                >
                                    <Trash2 size={13} />
                                </button>
                            )}
                        </div>
                    )}

                    {/* Songs in Active Collection */}
                    {!activeCollection || activeCollection.songs.length === 0 ? (
                        <div className="playlist-empty">
                            <p>{collections.length === 0 ? 'No collections yet' : 'No songs in this collection'}</p>
                            <p className="playlist-empty-hint">{collections.length === 0 ? 'Create your first collection above' : 'Add songs from search results'}</p>
                        </div>
                    ) : (
                        <>
                            <div className="playlist-bulk-toolbar">
                                <label className="playlist-select-visible">
                                    <input
                                        type="checkbox"
                                        checked={visibleSongs.length > 0 && selectedVisibleCount === visibleSongs.length}
                                        ref={(input) => {
                                            if (input) input.indeterminate = selectedVisibleCount > 0 && selectedVisibleCount < visibleSongs.length;
                                        }}
                                        onChange={(event) => handleSelectVisible(event.target.checked)}
                                    />
                                    <span>Select visible ({selectedVisibleCount}/{visibleSongs.length})</span>
                                </label>
                                <button className="btn-sm btn-secondary" onClick={handleBulkResolve} disabled={selectedResolvableSongs.length === 0}>
                                    Resolver/Re-resolver
                                </button>
                                <button className="btn-sm btn-primary" onClick={handleApplySelectedResolutions} disabled={selectedResolvableSongs.every((song) => !resolutionSuggestions[song.id]?.length)}>
                                    Aplicar propuestas
                                </button>
                                <select
                                    className="playlist-move-select"
                                    value={bulkMoveTargetId}
                                    onChange={(event) => setBulkMoveTargetId(event.target.value)}
                                    disabled={selectedSongIds.size === 0}
                                >
                                    <option value="">Mover a...</option>
                                    {collections.filter((collection) => collection.id !== activeCollection.id).map((collection) => (
                                        <option key={collection.id} value={collection.id}>{collection.name}</option>
                                    ))}
                                </select>
                                <button className="btn-sm btn-secondary" onClick={handleBulkMove} disabled={!bulkMoveTargetId || selectedSongIds.size === 0}>
                                    Mover
                                </button>
                                <button className="btn-sm btn-danger-text" onClick={handleBulkDelete} disabled={selectedSongIds.size === 0}>
                                    Borrar
                                </button>
                            </div>

                            <div className="playlist-page-info">
                                <span>
                                    {safePage * PLAYLIST_PAGE_SIZE + 1}-{Math.min((safePage + 1) * PLAYLIST_PAGE_SIZE, activeCollection.songs.length)}
                                    {' '}of {activeCollection.songs.length}
                                </span>
                                <div className="playlist-page-buttons">
                                    <button className="btn-sm btn-secondary" onClick={() => setPlaylistPage((page) => Math.max(0, page - 1))} disabled={safePage === 0}>
                                        Prev
                                    </button>
                                    <span>Page {safePage + 1} / {pageCount}</span>
                                    <button className="btn-sm btn-secondary" onClick={() => setPlaylistPage((page) => Math.min(pageCount - 1, page + 1))} disabled={safePage >= pageCount - 1}>
                                        Next
                                    </button>
                                </div>
                            </div>

                            <div className="playlist-list paged">
                                {visibleSongs.map((song, index) => (
                                    <PlaylistSongRow
                                        key={song.id}
                                        song={song}
                                        index={safePage * PLAYLIST_PAGE_SIZE + index}
                                        selected={selectedSongIds.has(song.id)}
                                        suggestions={resolutionSuggestions[song.id] || []}
                                        selectedSuggestionId={selectedSuggestions[song.id]}
                                        resolving={resolvingSongIds.has(song.id)}
                                        onSelect={handleSelectSong}
                                        onRemove={handleRemoveFromActiveCollection}
                                        onResolve={handleResolveImportedSong}
                                        onSuggestionChange={(songId, videoId) => setSelectedSuggestions((prev) => ({ ...prev, [songId]: videoId }))}
                                        onApplySuggestion={applyResolutionSuggestion}
                                    />
                                ))}
                            </div>
                        </>
                    )}
                </div>
            </div>
        </div>
    );
}
