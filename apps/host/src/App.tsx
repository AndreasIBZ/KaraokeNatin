import { useState, useEffect, useCallback, useRef, type FormEvent } from 'react';
import { init, useFocusable, FocusContext } from '@noriginmedia/norigin-spatial-navigation';
import Player from './components/Player';
import ControlPanel from './components/ControlPanel';
import GuestMode from './components/GuestMode';
import ModeSelect from './components/ModeSelect';
import Library from './components/Library';
import { useRoomState, setHostInputFocused } from './hooks/useRoomState';
import type { Song } from './hooks/useRoomState';
import { usePeerHost } from './hooks/usePeerHost';
import HelpDialog from './components/HelpDialog';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWebviewWindow, WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { listen } from '@tauri-apps/api/event';
import {
  startHostServer,
  openPlayerDisplay,
  closePlayerDisplay,
  shutdownHostSession,
  setPlayerDisplayFullscreen,
  getSessionHistory,
  loadCollectionFromFile,
  playlistImportCollection,
} from './lib/commands';
import { AlertTriangle, Clapperboard, SlidersHorizontal, Unplug, ArrowLeft, HelpCircle, X, Maximize2, Minimize2, Search, Plus, History } from 'lucide-react';

declare global {
  interface Window {
    __KARAOKE_SNAPSHOT_PLAYER_POSITION__?: () => Promise<void>;
  }
}

// Initialize spatial navigation for DPAD / Android TV
init({
  debug: false,
  visualDebug: false,
});

interface SearchResult {
  id: string;
  url: string;
  title: string;
  channel: string;
  duration: string;
  thumbnail: string;
}

type AppMode = 'select' | 'host' | 'guest' | 'library';

async function snapshotActivePlayerPosition() {
  await window.__KARAOKE_SNAPSHOT_PLAYER_POSITION__?.();
}

function secondsToDurationLabel(seconds: number) {
  if (!Number.isFinite(seconds) || seconds <= 0) return '';
  const mins = Math.floor(seconds / 60);
  const secs = seconds % 60;
  return `${mins}:${secs.toString().padStart(2, '0')}`;
}

function timeLabel(timestamp: number) {
  if (!Number.isFinite(timestamp)) return '';
  return new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' }).format(new Date(timestamp));
}

function songToSearchResult(song: Song): SearchResult {
  return {
    id: song.youtubeId,
    url: `https://www.youtube.com/watch?v=${song.youtubeId}`,
    title: song.title,
    channel: song.artist,
    duration: secondsToDurationLabel(song.duration),
    thumbnail: song.thumbnailUrl,
  };
}

function PlayerSearchDock({
  searching,
  searchResults,
  sessionHistory,
  karaokeOnly,
  onKaraokeOnlyChange,
  onSearch,
  onQueueResult,
  onQueueHistorySong,
}: {
  searching: boolean;
  searchResults: SearchResult[];
  sessionHistory: Song[];
  karaokeOnly: boolean;
  onKaraokeOnlyChange: (value: boolean) => void;
  onSearch: (query: string) => Promise<void> | void;
  onQueueResult: (result: SearchResult) => Promise<void>;
  onQueueHistorySong: (song: Song) => Promise<void>;
}) {
  const [query, setQuery] = useState('');
  const [isOpen, setIsOpen] = useState(false);
  const [queuedKey, setQueuedKey] = useState<string | null>(null);
  const [lastSubmittedQuery, setLastSubmittedQuery] = useState('');
  const dockRef = useRef<HTMLDivElement>(null);

  const recentHistory = sessionHistory.slice(-8).reverse();
  const showResults = searchResults.length > 0 || searching;
  const showHistory = isOpen && !showResults;

  useEffect(() => {
    if (!isOpen) return;

    const handlePointerDown = (event: PointerEvent) => {
      if (!dockRef.current?.contains(event.target as Node)) {
        setIsOpen(false);
      }
    };

    document.addEventListener('pointerdown', handlePointerDown);
    return () => document.removeEventListener('pointerdown', handlePointerDown);
  }, [isOpen]);

  const runSearch = async () => {
    const nextQuery = query.trim();
    if (!nextQuery) return;
    setIsOpen(true);
    setLastSubmittedQuery(nextQuery);
    await onSearch(nextQuery);
  };

  const handleSubmit = async (event?: FormEvent) => {
    event?.preventDefault();
    await runSearch();
  };

  const queueResult = async (result: SearchResult) => {
    setQueuedKey(result.url);
    try {
      await onQueueResult(result);
    } finally {
      window.setTimeout(() => setQueuedKey((current) => current === result.url ? null : current), 1200);
    }
  };

  const queueHistorySong = async (song: Song) => {
    setQueuedKey(song.id);
    try {
      await onQueueHistorySong(song);
    } finally {
      window.setTimeout(() => setQueuedKey((current) => current === song.id ? null : current), 1200);
    }
  };

  return (
    <div className="player-search-dock" ref={dockRef}>
      <form className={`player-search-form ${searching ? 'searching' : ''}`} onSubmit={handleSubmit}>
        <Search size={18} />
        <input
          className="player-search-input"
          value={query}
          placeholder="Search songs..."
          onChange={(event) => setQuery(event.target.value)}
          onFocus={() => {
            setIsOpen(true);
            setHostInputFocused(true);
          }}
          onBlur={() => {
            setHostInputFocused(false);
          }}
          onKeyDown={(event) => {
            if (event.key === 'Escape') {
              setIsOpen(false);
              return;
            }
            if (event.key === 'Enter') {
              event.preventDefault();
              event.stopPropagation();
              void runSearch();
            }
          }}
        />
        <label className="player-search-toggle">
          <input
            type="checkbox"
            checked={karaokeOnly}
            onChange={(event) => onKaraokeOnlyChange(event.target.checked)}
          />
          <span>Karaoke</span>
        </label>
        <button className="player-search-submit" type="submit" title={searching ? 'Search again' : 'Search'}>
          {searching ? <span className="spinner-tiny" /> : <Search size={16} />}
        </button>
      </form>

      {isOpen && (showResults || showHistory) && (
        <div className="player-search-popover">
          {searching && (
            <div className="player-search-state">
              <div className="spinner-tiny" /> Searching {lastSubmittedQuery ? `"${lastSubmittedQuery}"` : 'songs'}...
            </div>
          )}

          {!searching && searchResults.map((result) => (
            <div className="player-search-row" key={result.url}>
              <img src={result.thumbnail} alt="" className="player-search-thumb" />
              <div className="player-search-info">
                <div className="player-search-title">{result.title}</div>
                <div className="player-search-meta">{result.channel} • {result.duration}</div>
              </div>
              <button className="player-search-queue" onMouseDown={(event) => event.preventDefault()} onClick={() => queueResult(result)}>
                {queuedKey === result.url ? 'Queued' : <><Plus size={14} /> Queue</>}
              </button>
            </div>
          ))}

          {showHistory && (
            <>
              <div className="player-search-popover-title">
                <History size={14} /> Session history
              </div>
              {recentHistory.length === 0 ? (
                <div className="player-search-state">No songs queued in this session yet.</div>
              ) : recentHistory.map((song) => (
                <div className="player-search-row" key={song.id}>
                  <img src={song.thumbnailUrl} alt="" className="player-search-thumb" />
                  <div className="player-search-info">
                    <div className="player-search-title">{song.title}</div>
                    <div className="player-search-meta">
                      {song.addedBy || 'Host'} • {timeLabel(song.addedAt)}
                    </div>
                  </div>
                  <button className="player-search-queue" onMouseDown={(event) => event.preventDefault()} onClick={() => queueHistorySong(song)}>
                    {queuedKey === song.id ? 'Queued' : <><Plus size={14} /> Queue</>}
                  </button>
                </div>
              ))}
            </>
          )}
        </div>
      )}
    </div>
  );
}

// ---- Host-mode wrapper (hooks only active when rendered) ----
function HostView({ onBack }: { onBack: () => void }) {
  const { roomState, loading, initializeRoom } = useRoomState();
  const {
    connectionUrl,
    connectedClients,
    connectedClientList,
    pendingClientList,
    guestsCanInvite,
    setGuestsCanInvite,
    setClientCanReorderQueue,
    approveClient,
    rejectClient,
    kickClient,
  } = usePeerHost();
  const [isPanelCollapsed, setIsPanelCollapsed] = useState(false);
  const [searchResults, setSearchResults] = useState<SearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [sessionHistory, setSessionHistory] = useState<Song[]>([]);
  const [karaokeOnly, setKaraokeOnly] = useState(() => localStorage.getItem('karaoke_search_karaoke_only') !== 'false');
  const [isMobile, setIsMobile] = useState(false);
  const [activeTab, setActiveTab] = useState<'player' | 'controls'>('player');
  const [isPlayerDisplayOpen, setIsPlayerDisplayOpen] = useState(false);
  const searchRequestRef = useRef(0);

  const { ref, focusKey } = useFocusable();

  useEffect(() => {
    // Start web server lazily, then initialize room
    (async () => {
      try {
        await startHostServer();
      } catch (e) {
        console.warn('[Host] startHostServer:', e);
      }
      initializeRoom();
    })();
  }, []);

  // Detect mobile/small screen
  useEffect(() => {
    const checkMobile = () => {
      const mobile = window.innerWidth <= 768 || 'ontouchstart' in window;
      setIsMobile(mobile);
      if (mobile && window.innerHeight > window.innerWidth) {
        setActiveTab('controls');
      }
    };
    checkMobile();
    window.addEventListener('resize', checkMobile);
    return () => window.removeEventListener('resize', checkMobile);
  }, []);

  // Global DPAD handler for Android TV back button
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || e.key === 'GoBack') {
        if (!isPanelCollapsed && !isMobile) {
          setIsPanelCollapsed(true);
          e.preventDefault();
        }
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [isPanelCollapsed, isMobile]);

  useEffect(() => {
    localStorage.setItem('karaoke_search_karaoke_only', karaokeOnly ? 'true' : 'false');
  }, [karaokeOnly]);

  const refreshSessionHistory = useCallback(async () => {
    try {
      setSessionHistory(await getSessionHistory());
    } catch (error) {
      console.error('[Host] Failed to load session history:', error);
    }
  }, []);

  useEffect(() => {
    void refreshSessionHistory();
  }, [refreshSessionHistory, roomState?.player.currentSong?.id, roomState?.queue.length]);

  useEffect(() => {
    const unlistenOpened = listen('player-display-opened', () => {
      setIsPlayerDisplayOpen(true);
    });
    const unlistenClosed = listen('player-display-closed', () => {
      setIsPlayerDisplayOpen(false);
    });

    return () => {
      unlistenOpened.then((fn) => fn());
      unlistenClosed.then((fn) => fn());
    };
  }, []);

  const handleSearch = async (query: string) => {
    const requestId = searchRequestRef.current + 1;
    searchRequestRef.current = requestId;
    setSearching(true);
    setSearchResults([]);
    try {
      const results = await invoke<SearchResult[]>('search_youtube', { query, limit: 10, karaokeOnly });
      if (requestId === searchRequestRef.current) {
        setSearchResults(results);
      }
    } catch (error) {
      console.error('Search failed:', error);
    } finally {
      if (requestId === searchRequestRef.current) {
        setSearching(false);
      }
    }
  };

  const handleQueueSearchResult = useCallback(async (result: SearchResult) => {
    await invoke('queue_search_result', { result, addedBy: 'Host' });
    await refreshSessionHistory();
  }, [refreshSessionHistory]);

  const handleQueueHistorySong = useCallback(async (song: Song) => {
    await handleQueueSearchResult(songToSearchResult(song));
  }, [handleQueueSearchResult]);

  const handleAddToPlaylist = async (url: string, collectionId: string) => {
    await invoke('process_command', {
      command: {
        type: 'PLAYLIST_ADD',
        youtubeUrl: url,
        collectionId,
        addedBy: 'Host',
      },
    });
  };

  const handleTabSwitch = useCallback((tab: 'player' | 'controls') => {
    setActiveTab(tab);
  }, []);

  const handleOpenPlayerDisplay = useCallback(async () => {
    await snapshotActivePlayerPosition();
    await openPlayerDisplay();
    setIsPlayerDisplayOpen(true);
  }, []);

  const handleClosePlayerDisplay = useCallback(async () => {
    if (!isPlayerDisplayOpen) {
      await snapshotActivePlayerPosition();
      await closePlayerDisplay();
      setIsPlayerDisplayOpen(false);
      return;
    }

    try {
      await getCurrentWebviewWindow().emitTo('player-display', 'player-display-return-requested');
    } catch (error) {
      console.warn('[Host] Could not request player display return, closing directly:', error);
      await closePlayerDisplay();
      setIsPlayerDisplayOpen(false);
    }
  }, [isPlayerDisplayOpen]);

  const handleFullscreenPlayerDisplay = useCallback(async () => {
    await setPlayerDisplayFullscreen(true);
  }, []);

  if (loading) {
    return (
      <div className="app-loading">
        <div className="app-loading-title">FESTEJAR</div>
        <div className="app-loading-spinner"></div>
        <div className="app-loading-hint">Starting host…</div>
      </div>
    );
  }

  return (
    <FocusContext.Provider value={focusKey}>
      <div ref={ref} className={`app-container ${isMobile ? 'mobile' : 'desktop'}`}>
        {/* Mobile Tab Bar */}
        {isMobile && (
          <div className="mobile-tab-bar">
            <button className={`mobile-tab ${activeTab === 'player' ? 'active' : ''}`} onClick={() => handleTabSwitch('player')}>
              <Clapperboard size={16} /> Player
            </button>
            <button className={`mobile-tab ${activeTab === 'controls' ? 'active' : ''}`} onClick={() => handleTabSwitch('controls')}>
              <SlidersHorizontal size={16} /> Controls
            </button>
          </div>
        )}

        {/* Main player area */}
        <div className={`main-area ${isMobile && activeTab !== 'player' ? 'hidden-mobile' : ''}`}>
          <PlayerSearchDock
            searching={searching}
            searchResults={searchResults}
            sessionHistory={sessionHistory}
            karaokeOnly={karaokeOnly}
            onKaraokeOnlyChange={setKaraokeOnly}
            onSearch={handleSearch}
            onQueueResult={handleQueueSearchResult}
            onQueueHistorySong={handleQueueHistorySong}
          />
          {isPlayerDisplayOpen ? (
            <div className="detached-player-placeholder">
              <div className="detached-player-kicker">Player Display activo</div>
              <h2>El video esta en la pantalla secundaria</h2>
              {roomState?.player.currentSong ? (
                <p>
                  {roomState.player.currentSong.title}
                  {' '}· Added by {roomState.player.currentSong.addedBy}
                </p>
              ) : (
                <p>No hay ninguna cancion reproduciendose ahora mismo.</p>
              )}
            </div>
          ) : (
            <Player roomState={roomState} />
          )}
          <footer className="yt-footer-host">
            Videos are played via YouTube embedding. All videos are subject to YouTube's{' '}
            <a href="https://www.youtube.com/t/terms" target="_blank" rel="noopener noreferrer">Terms of Service</a>.
          </footer>
        </div>

        {/* Control panel / sidebar */}
        <div className={`panel-wrapper ${isMobile && activeTab !== 'controls' ? 'hidden-mobile' : ''}`}>
          <ControlPanel
            connectionUrl={connectionUrl}
            roomId={roomState?.roomId}
            queue={roomState?.queue || []}
            playlists={roomState?.playlists || []}
            connectedClients={connectedClients}
            connectedClientList={connectedClientList}
            pendingClientList={pendingClientList}
            guestsCanInvite={guestsCanInvite}
            onGuestsCanInviteChange={setGuestsCanInvite}
            onClientCanReorderQueueChange={setClientCanReorderQueue}
            onApproveClient={approveClient}
            onRejectClient={rejectClient}
            onKickClient={kickClient}
            isCollapsed={isMobile ? false : isPanelCollapsed}
            onToggle={() => setIsPanelCollapsed(!isPanelCollapsed)}
            onSearch={handleSearch}
            karaokeOnly={karaokeOnly}
            onKaraokeOnlyChange={setKaraokeOnly}
            searchResults={searchResults}
            searching={searching}
            onAddToPlaylist={handleAddToPlaylist}
            playerDisplayOpen={isPlayerDisplayOpen}
            onOpenPlayerDisplay={handleOpenPlayerDisplay}
            onClosePlayerDisplay={handleClosePlayerDisplay}
            onFullscreenPlayerDisplay={handleFullscreenPlayerDisplay}
            isPlaying={roomState?.player.status === 'playing'}
            currentSong={roomState?.player.currentSong || null}
            volume={roomState?.player.volume}
            isMuted={roomState?.player.isMuted}
            currentTime={roomState?.player.currentTime}
            duration={roomState?.player.duration}
            isMobile={isMobile}
            onBack={onBack}
          />
        </div>
      </div>
    </FocusContext.Provider>
  );
}

function PlayerDisplayView() {
  const { roomState, loading, refreshRoomState } = useRoomState();
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const closingRef = useRef(false);

  useEffect(() => {
    void refreshRoomState().catch((error) => {
      console.error('[PlayerDisplay] Failed to load room state:', error);
      setLoadError(error instanceof Error ? error.message : String(error));
    });
  }, [refreshRoomState]);

  useEffect(() => {
    const window = getCurrentWebviewWindow();
    const unlisten = window.onCloseRequested(async (event) => {
      if (closingRef.current) return;
      event.preventDefault();
      closingRef.current = true;
      await snapshotActivePlayerPosition();
      await closePlayerDisplay();
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    const currentWindow = getCurrentWebviewWindow();
    const interval = window.setInterval(async () => {
      const mainWindow = await WebviewWindow.getByLabel('main');
      if (!mainWindow) {
        await currentWindow.destroy();
      }
    }, 1000);

    return () => {
      window.clearInterval(interval);
    };
  }, []);

  const handleClose = useCallback(async () => {
    if (closingRef.current) return;
    closingRef.current = true;
    await snapshotActivePlayerPosition();
    await closePlayerDisplay();
  }, []);

  useEffect(() => {
    const handleKeyDown = async (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (isFullscreen) {
        await setPlayerDisplayFullscreen(false);
        setIsFullscreen(false);
      } else {
        await handleClose();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleClose, isFullscreen]);

  useEffect(() => {
    const unlisten = listen('player-display-return-requested', () => {
      void handleClose();
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [handleClose]);

  const handleToggleFullscreen = useCallback(async () => {
    const next = !isFullscreen;
    await setPlayerDisplayFullscreen(next);
    setIsFullscreen(next);
  }, [isFullscreen]);

  return (
    <div className="player-display-window">
      <div className="player-display-toolbar">
        <button
          className="player-display-tool"
          onClick={handleToggleFullscreen}
          title={isFullscreen ? 'Salir de pantalla completa' : 'Pantalla completa'}
        >
          {isFullscreen ? <Minimize2 size={18} /> : <Maximize2 size={18} />}
        </button>
        <button
          className="player-display-tool"
          onClick={handleClose}
          title="Cerrar pantalla"
        >
          <X size={18} />
        </button>
      </div>
      {loadError ? (
        <div className="app-loading">
          <div className="app-loading-title">FESTEJAR</div>
          <div className="app-loading-hint">No se pudo cargar el player display.</div>
          <div className="app-loading-hint">{loadError}</div>
        </div>
      ) : loading && !roomState ? (
        <div className="app-loading">
          <div className="app-loading-title">FESTEJAR</div>
          <div className="app-loading-spinner"></div>
          <div className="app-loading-hint">Opening player display...</div>
        </div>
      ) : (
        <Player roomState={roomState} hideFullscreenButton />
      )}
    </div>
  );
}

// ---- Guest-mode wrapper ----
function GuestView({ onBack }: { onBack: () => void }) {
  const [guestHostUrl, setGuestHostUrl] = useState<string | null>(null);
  const [guestIframeLoaded, setGuestIframeLoaded] = useState(false);
  const [showScanner, setShowScanner] = useState(true);
  const guestIframeRef = useRef<HTMLIFrameElement>(null);

  const handleGuestConnect = useCallback((hostUrl: string) => {
    const url = new URL(hostUrl);
    url.searchParams.set('mode', 'inapp');
    // Cache-buster. Named `_cb`, not `t`: `t` carries the join token from the
    // scanned QR URL, and overwriting it with a timestamp would make every
    // in-app guest connection fail token verification.
    url.searchParams.set('_cb', Date.now().toString());
    setGuestHostUrl(url.toString());
    setGuestIframeLoaded(false);
    setShowScanner(false);
  }, []);

  const handleGuestDisconnect = useCallback(() => {
    setGuestHostUrl(null);
    setGuestIframeLoaded(false);
    setShowScanner(true);
  }, []);

  // Bridge for Remote UI to access Native features (My Library)
  useEffect(() => {
    const handleMessage = async (event: MessageEvent) => {
      // Security check: ensure message is from our iframe
      if (!guestIframeRef.current || event.source !== guestIframeRef.current.contentWindow) {
        return;
      }

      const { type, payload } = event.data;

      try {
        if (type === 'REQUEST_LOCAL_PLAYLISTS') {
          const playlists = await invoke('get_playlists');
          guestIframeRef.current.contentWindow?.postMessage({
            type: 'LOCAL_PLAYLISTS_UPDATED',
            playlists
          }, '*');
        }
        else if (type === 'CREATE_LOCAL_COLLECTION') {
          await invoke('playlist_create_collection', {
            name: payload.name,
            visibility: 'personal' // Guest collections are personal by default
          });
          // Refresh
          const playlists = await invoke('get_playlists');
          guestIframeRef.current.contentWindow?.postMessage({
            type: 'LOCAL_PLAYLISTS_UPDATED',
            playlists
          }, '*');
        }
        else if (type === 'ADD_TO_LOCAL_PLAYLIST') {
          await invoke('playlist_add_song', {
            youtubeUrl: payload.youtubeUrl,
            collectionId: payload.collectionId,
            addedBy: 'Guest' // Or user's name if we had it, but 'Guest' is safer for now
          });
          // Refresh
          const playlists = await invoke('get_playlists');
          guestIframeRef.current.contentWindow?.postMessage({
            type: 'LOCAL_PLAYLISTS_UPDATED',
            playlists
          }, '*');
          // Also send success toast trigger
          guestIframeRef.current.contentWindow?.postMessage({
            type: 'TOAST',
            message: 'Saved to library'
          }, '*');
        }
        else if (type === 'REMOVE_FROM_LOCAL_PLAYLIST') {
          await invoke('playlist_remove_song', {
            collectionId: payload.collectionId,
            songId: payload.songId
          });
          // Refresh
          const playlists = await invoke('get_playlists');
          guestIframeRef.current.contentWindow?.postMessage({
            type: 'LOCAL_PLAYLISTS_UPDATED',
            playlists
          }, '*');
        }
        else if (type === 'IMPORT_LOCAL_PLAYLIST') {
          const json = await loadCollectionFromFile();
          if (json) {
            // Rust names this parameter `data`, not `json`. Routed through the
            // typed wrapper so the name lives in exactly one place.
            await playlistImportCollection(json);
            const playlists = await invoke('get_playlists');
            guestIframeRef.current.contentWindow?.postMessage({
              type: 'LOCAL_PLAYLISTS_UPDATED',
              playlists
            }, '*');
            guestIframeRef.current.contentWindow?.postMessage({
              type: 'TOAST',
              message: 'Collection imported'
            }, '*');
          }
        }
        else if (type === 'EXPORT_LOCAL_PLAYLIST') {
          await invoke('save_collection_to_file', {
            collectionId: payload.collectionId
          });
        }
      } catch (err) {
        console.error('[GuestView] Bridge error:', err);
      }
    };

    window.addEventListener('message', handleMessage);
    return () => window.removeEventListener('message', handleMessage);
  }, []);

  return (
    <div className="guest-view">
      {guestHostUrl ? (
        <div className="guest-iframe-area">
          <div className="guest-banner">
            <span className="guest-banner-label">
              {guestIframeLoaded ? 'Connected to remote host' : 'Connecting...'}
            </span>
            <button className="btn-sm btn-secondary guest-banner-btn" onClick={handleGuestDisconnect}>
              <Unplug size={14} /> Disconnect
            </button>
          </div>
          {!guestIframeLoaded && (
            <div className="guest-iframe-loading">
              <div className="spinner" />
              <p>Loading remote control...</p>
            </div>
          )}
          <iframe
            ref={guestIframeRef}
            src={guestHostUrl}
            className="guest-inline-iframe"
            style={{ opacity: guestIframeLoaded ? 1 : 0 }}
            onLoad={() => setGuestIframeLoaded(true)}
            title="Remote Control"
            allow="microphone; camera"
          />
          <footer className="yt-footer-host">
            Videos are played via YouTube embedding. All videos are subject to YouTube's{' '}
            <a href="https://www.youtube.com/t/terms" target="_blank" rel="noopener noreferrer">Terms of Service</a>.
          </footer>
        </div>
      ) : showScanner ? (
        <div className="guest-scan-wrapper">
          <button className="mode-back-btn" onClick={onBack}>
            <ArrowLeft size={16} /> Back
          </button>
          <GuestMode
            onClose={onBack}
            onConnect={handleGuestConnect}
          />
        </div>
      ) : null}
    </div>
  );
}

// ---- Root App ----
function App() {
  const windowLabel = getCurrentWebviewWindow().label;
  const isPlayerDisplayWindow =
    windowLabel === 'player-display' ||
    Boolean((window as typeof window & { __KARAOKE_PLAYER_DISPLAY__?: boolean }).__KARAOKE_PLAYER_DISPLAY__);
  const [appMode, setAppMode] = useState<AppMode>('select');
  const [showHelp, setShowHelp] = useState(false);
  const [showHostExitConfirm, setShowHostExitConfirm] = useState(false);

  if (isPlayerDisplayWindow) {
    return <PlayerDisplayView />;
  }

  const handleBack = useCallback(() => {
    setShowHostExitConfirm(false);
    setAppMode('select');
  }, []);
  const handleHostBack = useCallback(() => {
    setShowHostExitConfirm(true);
  }, []);

  const confirmHostExit = useCallback(async () => {
    setShowHostExitConfirm(false);
    try {
      await shutdownHostSession();
    } catch (error) {
      console.warn('[App] Failed to shutdown host session cleanly:', error);
      try {
        await closePlayerDisplay();
      } catch (closeError) {
        console.warn('[App] Failed to close player display during host exit fallback:', closeError);
      }
    }
    setAppMode('select');
  }, []);

  const cancelHostExit = useCallback(() => {
    setShowHostExitConfirm(false);
  }, []);

  const exitDialog = showHostExitConfirm
    ? {
        title: 'Salir del modo Host',
        body: 'Esto cerrara la sesion actual, desconectara a los usuarios conectados e invalidara el enlace de invitacion.',
        confirmLabel: 'Salir del Host',
        onCancel: cancelHostExit,
        onConfirm: confirmHostExit,
      }
    : null;

  if (appMode === 'select') {
    return (
      <>
        <ModeSelect
          onSelectHost={() => setAppMode('host')}
          onSelectGuest={() => setAppMode('guest')}
          onSelectLibrary={() => setAppMode('library')}
        />
        <button
          className="help-fab"
          onClick={() => setShowHelp(true)}
          aria-label="Help and diagnostics"
          title="Help and diagnostics"
        >
          <HelpCircle size={22} />
        </button>
        {showHelp && <HelpDialog onClose={() => setShowHelp(false)} />}
      </>
    );
  }

  if (appMode === 'host') {
    return (
      <>
        <HostView onBack={handleHostBack} />
        {exitDialog && (
          <div
            className="host-exit-overlay"
            role="dialog"
            aria-modal="true"
            aria-labelledby="host-exit-title"
          >
            <div className="host-exit-dialog">
              <div className="host-exit-icon">
                <AlertTriangle size={24} />
              </div>
              <h2 id="host-exit-title">{exitDialog.title}</h2>
              <p>{exitDialog.body}</p>
              <div className="host-exit-actions">
                <button className="btn-sm btn-secondary" onClick={exitDialog.onCancel}>
                  Cancelar
                </button>
                <button className="btn-sm btn-danger" onClick={exitDialog.onConfirm}>
                  {exitDialog.confirmLabel}
                </button>
              </div>
            </div>
          </div>
        )}
      </>
    );
  }

  if (appMode === 'library') {
    return <Library onBack={handleBack} />;
  }

  return <GuestView onBack={handleBack} />;
}

export default App;
