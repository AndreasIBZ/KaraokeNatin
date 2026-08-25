import { useCallback, useState } from 'react';
import { Upload, X } from 'lucide-react';
import {
    confirmPlaylistImport,
    previewKaraokeJsonPlaylistImport,
    previewSpotifyPlaylistImport,
    previewTextPlaylistImport,
    type ImportPreview,
} from '../lib/commands';
import { setHostInputFocused } from '../hooks/useRoomState';

interface PlaylistImportModalProps {
    onClose: () => void;
    onImported: (collectionId: string) => void | Promise<void>;
}

async function readFileAsText(file: File) {
    return await new Promise<string>((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result || ''));
        reader.onerror = () => reject(reader.error || new Error('Failed to read file'));
        reader.readAsText(file);
    });
}

function isSpotifyPlaylistUrl(value: string) {
    try {
        const url = new URL(value.trim());
        const parts = url.pathname.split('/').filter(Boolean);
        return url.protocol === 'https:' &&
            url.hostname === 'open.spotify.com' &&
            parts.length === 2 &&
            parts[0] === 'playlist' &&
            /^[A-Za-z0-9]+$/.test(parts[1]);
    } catch {
        return false;
    }
}

export default function PlaylistImportModal({ onClose, onImported }: PlaylistImportModalProps) {
    const [importText, setImportText] = useState('');
    const [importPreview, setImportPreview] = useState<ImportPreview | null>(null);
    const [importing, setImporting] = useState(false);
    const [importError, setImportError] = useState<string | null>(null);

    const resetAndClose = useCallback(() => {
        if (importing) return;
        setImportPreview(null);
        setImportError(null);
        setImportText('');
        onClose();
    }, [importing, onClose]);

    const previewInput = useCallback(async (data: string, sourceName?: string) => {
        const clean = data.trim();
        if (!clean) {
            setImportError('Pega canciones, enlaces de video o una URL de playlist de Spotify.');
            return;
        }

        setImporting(true);
        setImportError(null);
        try {
            if (sourceName?.endsWith('.karaoke.json')) {
                setImportPreview(await previewKaraokeJsonPlaylistImport(clean));
            } else if (isSpotifyPlaylistUrl(clean)) {
                setImportPreview(await previewSpotifyPlaylistImport(clean));
            } else {
                setImportPreview(await previewTextPlaylistImport(clean));
            }
        } catch (error) {
            console.error('[PlaylistImportModal] Preview failed:', error);
            setImportError(typeof error === 'string' ? error : 'No se pudo preparar la importacion.');
        } finally {
            setImporting(false);
        }
    }, []);

    const previewFile = useCallback(async (file: File) => {
        const lowerName = file.name.toLowerCase();
        if (!lowerName.endsWith('.karaoke.json') && !lowerName.endsWith('.txt')) {
            setImportError('Formato de importacion no reconocido.');
            return;
        }

        try {
            const text = await readFileAsText(file);
            await previewInput(text, lowerName);
        } catch (error) {
            console.error('[PlaylistImportModal] File read failed:', error);
            setImportError('No se pudo leer el archivo.');
        }
    }, [previewInput]);

    const handleConfirmImport = useCallback(async (updateExisting = false) => {
        if (!importPreview) return;
        setImporting(true);
        setImportError(null);
        try {
            const collectionId = await confirmPlaylistImport(importPreview.playlist, updateExisting);
            await onImported(collectionId);
            resetAndClose();
        } catch (error) {
            console.error('[PlaylistImportModal] Confirm import failed:', error);
            setImportError(typeof error === 'string' ? error : 'No se pudo importar la playlist.');
        } finally {
            setImporting(false);
        }
    }, [importPreview, onImported, resetAndClose]);

    return (
        <div className="playlist-import-overlay" role="dialog" aria-modal="true" onClick={resetAndClose}>
            <div className="playlist-import-modal" onClick={(event) => event.stopPropagation()}>
                <div className="playlist-import-header">
                    <div>
                        <div className="playlist-import-title">Import Playlist</div>
                        <p>.karaoke.json, .txt, Spotify URL, YouTube video links or plain titles</p>
                    </div>
                    <button className="btn-icon" onClick={resetAndClose} title="Close" disabled={importing}>
                        <X size={16} />
                    </button>
                </div>

                <div className="playlist-import-body">
                    <label className="playlist-import-file">
                        <Upload size={18} />
                        <span>Choose playlist file</span>
                        <input
                            type="file"
                            accept=".karaoke.json,.txt,application/json,text/plain"
                            onChange={(event) => {
                                const file = event.target.files?.[0];
                                if (file) void previewFile(file);
                                event.currentTarget.value = '';
                            }}
                            disabled={importing}
                        />
                    </label>

                    <div className="playlist-import-divider">or</div>

                    <form
                        className="playlist-import-url"
                        onSubmit={(event) => {
                            event.preventDefault();
                            void previewInput(importText);
                        }}
                    >
                        <span>Paste playlist data</span>
                        <textarea
                            className="playlist-import-textarea"
                            value={importText}
                            onChange={(event) => setImportText(event.target.value)}
                            onFocus={() => setHostInputFocused(true)}
                            onBlur={() => setHostInputFocused(false)}
                            placeholder={'https://open.spotify.com/playlist/...\nQueen - Radio Ga Ga\nQueen - We Are The Champions | https://youtu.be/...'}
                            rows={7}
                            disabled={importing}
                        />
                        <div className="playlist-import-actions">
                            <button className="btn-sm btn-primary" disabled={importing}>
                                {importing ? 'Loading...' : 'Preview'}
                            </button>
                        </div>
                    </form>

                    {importError && <div className="playlist-import-error">{importError}</div>}

                    {importPreview && (
                        <div className="playlist-import-preview">
                            <div className="playlist-import-detected">Playlist detectada</div>
                            <h3>{importPreview.playlist.name}</h3>
                            <p>
                                {importPreview.validTrackCount} tracks ready
                                {importPreview.incompleteTrackCount > 0 ? `, ${importPreview.incompleteTrackCount} skipped` : ''}
                            </p>
                            {importPreview.existingCollectionId && (
                                <p className="playlist-import-note">Ya existe localmente. Puedes actualizarla.</p>
                            )}
                            <ol className="playlist-import-track-list">
                                {importPreview.playlist.tracks.slice(0, 8).map((track, index) => (
                                    <li key={`${track.title}-${index}`}>
                                        <span>{track.title}</span>
                                        <small>{track.artists.join(', ') || 'Unknown Artist'}</small>
                                    </li>
                                ))}
                            </ol>
                            <button
                                className="btn-primary"
                                onClick={() => void handleConfirmImport(!!importPreview.existingCollectionId)}
                                disabled={importing}
                            >
                                {importPreview.existingCollectionId ? 'Actualizar' : 'Importar'}
                            </button>
                        </div>
                    )}
                </div>
            </div>
        </div>
    );
}
