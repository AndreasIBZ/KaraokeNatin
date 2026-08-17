import { useState, useEffect, useRef } from 'react';
import Peer, { DataConnection } from 'peerjs';
import { io, Socket } from 'socket.io-client';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { ClientCommand, HostBroadcast, isClientCommand, RoomState } from '@karaokenatin/shared';
import { processCommand, getRoomState, startHostServer } from '../lib/commands';
import { hashToken, generateRoomId, generateJoinToken } from '../lib/security';

export interface ConnectedPeer {
    id: string;
    displayName: string;
    canReorderQueue: boolean;
}

export interface PendingPeer {
    id: string;
    displayName: string;
    previouslyKicked: boolean;
}

/**
 * Hook to manage PeerJS host and WebRTC connections
 */
export function usePeerHost() {
    const [peer, setPeer] = useState<Peer | null>(null);
    const [connections, setConnections] = useState<Map<string, DataConnection>>(new Map());
    const connectionsRef = useRef<Map<string, DataConnection>>(new Map());
    const [pendingConnections, setPendingConnections] = useState<Map<string, DataConnection>>(new Map());
    const pendingConnectionsRef = useRef<Map<string, DataConnection>>(new Map());
    const [clientNames, setClientNames] = useState<Map<string, string>>(new Map());
    const clientNamesRef = useRef<Map<string, string>>(new Map());
    const [pendingClientNames, setPendingClientNames] = useState<Map<string, string>>(new Map());
    const pendingClientNamesRef = useRef<Map<string, string>>(new Map());
    const clientKeysRef = useRef<Map<string, string>>(new Map());
    const pendingClientKeysRef = useRef<Map<string, string>>(new Map());
    const approvedClientKeysRef = useRef<Set<string>>(new Set());
    const blockedClientKeysRef = useRef<Set<string>>(new Set());
    const [kickedClientNames, setKickedClientNames] = useState<Set<string>>(new Set());
    const kickedClientNamesRef = useRef<Set<string>>(new Set());
    const [guestsCanInvite, setGuestsCanInvite] = useState(true);
    const guestsCanInviteRef = useRef(true);
    const [reorderAllowedPeerIds, setReorderAllowedPeerIds] = useState<Set<string>>(new Set());
    const reorderAllowedPeerIdsRef = useRef<Set<string>>(new Set());
    const reorderAllowedClientKeysRef = useRef<Set<string>>(new Set());
    const [connectionUrl, setConnectionUrl] = useState<string>('');
    // Held in state so the socket survives re-renders; the cleanup path uses the
    // local `socketInstance` binding instead, so this value is write-only.
    const [, setSocket] = useState<Socket | null>(null);

    // Keep ref in sync with state
    useEffect(() => {
        connectionsRef.current = connections;
    }, [connections]);

    useEffect(() => {
        pendingConnectionsRef.current = pendingConnections;
    }, [pendingConnections]);

    useEffect(() => {
        kickedClientNamesRef.current = kickedClientNames;
    }, [kickedClientNames]);

    useEffect(() => {
        guestsCanInviteRef.current = guestsCanInvite;
        connectionsRef.current.forEach((conn, peerId) => sendSessionSettings(peerId, conn));
    }, [guestsCanInvite]);

    useEffect(() => {
        reorderAllowedPeerIdsRef.current = reorderAllowedPeerIds;
        connectionsRef.current.forEach((conn, peerId) => sendSessionSettings(peerId, conn));
    }, [reorderAllowedPeerIds]);

    // Sweep for channels that went away without firing 'close'. PeerJS does not
    // reliably emit close when the underlying transport dies (a slept phone, a
    // dropped access point), so `conn.open` is the only honest signal.
    useEffect(() => {
        const REAP_INTERVAL_MS = 15000;
        const timer = setInterval(() => {
            const stale: string[] = [];
            connectionsRef.current.forEach((conn, peerId) => {
                if (!conn.open) stale.push(peerId);
            });
            const stalePending: string[] = [];
            pendingConnectionsRef.current.forEach((conn, peerId) => {
                if (!conn.open) stalePending.push(peerId);
            });
            if (stale.length === 0 && stalePending.length === 0) return;
            if (stale.length > 0) {
                console.log('[PeerHost] Reaping stale active connections:', stale);
                stale.forEach(dropConnection);
            }
            if (stalePending.length > 0) {
                console.log('[PeerHost] Reaping stale pending connections:', stalePending);
                stalePending.forEach(dropPendingConnection);
            }
        }, REAP_INTERVAL_MS);

        return () => clearInterval(timer);
    }, []);

    // Subscribe to room state updates and broadcast to all connected peers
    useEffect(() => {
        // `room_state_public` is emitted by Rust with personal collections
        // already stripped (see emit_state in commands.rs). Do not switch this
        // to `room_state_updated` — that carries the host's private playlists
        // and this handler forwards its payload straight to every guest.
        const broadcast = (message: HostBroadcast) => {
            connectionsRef.current.forEach((conn) => {
                if (conn.open) {
                    conn.send(message);
                }
            });
        };

        const unlistenFull = listen<RoomState>('room_state_public', (event) => {
            broadcast({ type: 'STATE_UPDATE', state: event.payload });
        });

        // Player progress ticks arrive several times a minute and previously
        // resent the entire room — queue plus every public collection — to
        // every guest just to move a timestamp. `player` is a self-contained
        // subtree, so patching it cannot desync the rest.
        const unlistenPlayer = listen<RoomState['player']>('room_player_patch', (event) => {
            broadcast({ type: 'STATE_PATCH', patch: { player: event.payload } });
        });

        return () => {
            unlistenFull.then(fn => fn());
            unlistenPlayer.then(fn => fn());
        };
    }, []);

    useEffect(() => {
        let cancelled = false;
        let peerInstance: Peer | null = null;
        let socketInstance: Socket | null = null;

        const setup = async () => {
        // The broker lives in our own Rust web server, so start it here before
        // constructing PeerJS. HostView also starts it for room setup; the Rust
        // command is idempotent, and doing it here avoids a first-render race
        // where this hook could read port 0 and create an unusable room.
        let port = await startHostServer();
        if (!port) {
            port = await invoke<number>('get_server_port');
        }
        if (!port) {
            throw new Error('Host server did not report a usable port');
        }
        if (cancelled) return;

        // Point PeerJS at that broker. Omitting host/port/path makes PeerJS
        // fall back to its public 0.peerjs.com cloud, which put the WebRTC
        // handshake on the internet and made this LAN app unusable offline.
        // `path: '/'` is correct: PeerJS appends 'peerjs', giving '/peerjs'.
        const hostPeerId = `host-${generateJoinToken()}`;
        peerInstance = new Peer(hostPeerId, {
            host: 'localhost',
            port,
            path: '/',
            key: 'peerjs',
            secure: false,
            config: {
                iceServers: [
                    { urls: 'stun:stun.l.google.com:19302' },
                    { urls: 'stun:stun1.l.google.com:19302' },
                ],
            },
        });

        peerInstance.on('open', async (peerId) => {
            console.log('[PeerHost] Peer ID:', peerId);

            // Generate room credentials
            const roomId = generateRoomId();
            const joinToken = generateJoinToken();
            const joinTokenHash = await hashToken(joinToken);

            // Connect to signaling server (same embedded server, same port).
            // Tauri's release WebView is not same-origin with this HTTP server,
            // so Socket.IO's default polling preflight can be blocked by CORS
            // before the room is created. WebSocket avoids that browser CORS
            // path and is the only transport we need on localhost.
            socketInstance = io(`http://localhost:${port}`, {
                transports: ['websocket'],
            });

            // Include peerId when creating room so clients can connect
            socketInstance.emit('CREATE_ROOM', { roomId, joinTokenHash, hostPeerId: peerId });

            socketInstance.on('ROOM_CREATED', async () => {
                console.log('[PeerHost] Room created on signaling server');
                // Get the base URL (http://ip:port) from the backend
                try {
                    const baseUrl = await invoke<string>('get_qr_url');
                    // The token rides in the QR URL. remote-ui reads ?t= and
                    // sends it as joinToken, which the signaling server now
                    // verifies for every join (see signaling.rs JOIN_ROOM).
                    // Without this the token would be unusable and the server
                    // would have to accept anonymous joins.
                    setConnectionUrl(`${baseUrl}/?t=${encodeURIComponent(joinToken)}`);
                } catch (e) {
                    console.error('Failed to get QR URL:', e);
                    setConnectionUrl(`${window.location.origin}/?t=${encodeURIComponent(joinToken)}`);
                }
            });

            setSocket(socketInstance);
        });

        peerInstance.on('connection', (conn) => {
            console.log('[PeerHost] New peer connection:', conn.peer);
            setupDataChannelHandlers(conn);
        });

        peerInstance.on('error', (err) => {
            console.error('[PeerHost] Peer error:', err);
        });

        setPeer(peerInstance);
        };

        setup().catch((e) => console.error('[PeerHost] Setup failed:', e));

        return () => {
            // Guards against the effect's async setup finishing after unmount,
            // which would otherwise leave an orphaned Peer and socket alive.
            cancelled = true;
            peerInstance?.destroy();
            socketInstance?.disconnect();
        };
    }, []);

    const setupDataChannelHandlers = (conn: DataConnection) => {
        conn.on('open', () => {
            console.log('[PeerHost] DataChannel open:', conn.peer);
            setPendingConnections((prev) => new Map(prev).set(conn.peer, conn));
            ensurePendingClientName(conn.peer);
            sendWaitingApproval(conn.peer, conn);
        });

        conn.on('data', async (data) => {
            console.log('[PeerHost] Received data:', data);

            // Handle SEARCH command separately (not a standard ClientCommand)
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
            const msg = data as Record<string, any>;

            if (msg && msg.type === 'SET_DISPLAY_NAME' && typeof msg.name === 'string') {
                const clientKey = cleanClientKey(msg.clientKey);
                if (connectionsRef.current.has(conn.peer)) {
                    rememberClientName(conn.peer, msg.name);
                    rememberClientKey(conn.peer, clientKey);
                } else {
                    rememberPendingClientName(conn.peer, msg.name);
                    rememberPendingClientKey(conn.peer, clientKey);
                    if (
                        clientKey &&
                        approvedClientKeysRef.current.has(clientKey) &&
                        !blockedClientKeysRef.current.has(clientKey)
                    ) {
                        approveClient(conn.peer);
                    } else {
                        sendWaitingApproval(conn.peer, conn);
                    }
                }
                return;
            }

            if (msg && msg.type === 'SEARCH' && typeof msg.query === 'string') {
                console.log('[PeerHost] Processing SEARCH:', msg.query);
                try {
                    const results = await invoke('search_youtube', {
                        query: msg.query,
                        limit: msg.limit || 5,
                        karaokeOnly: msg.karaokeOnly !== false,
                    });
                    conn.send({ type: 'SEARCH_RESULTS', results });
                } catch (error) {
                    console.error('[PeerHost] Search failed:', error);
                    conn.send({
                        type: 'ERROR',
                        code: 'SEARCH_FAILED',
                        message: typeof error === 'string' ? error : error instanceof Error ? error.message : 'Search failed'
                    });
                }
                return;
            }

            if (msg && msg.type === 'ADD_SEARCH_RESULT' && msg.result) {
                console.log('[PeerHost] Queueing search result:', msg.result);
                try {
                    const actorName = getClientName(conn.peer, msg.addedBy);
                    await invoke('queue_search_result', {
                        result: msg.result,
                        addedBy: actorName,
                    });
                } catch (error) {
                    console.error('[PeerHost] Add search result failed:', error);
                    conn.send({
                        type: 'ERROR',
                        code: 'COMMAND_FAILED',
                        message: typeof error === 'string' ? error : error instanceof Error ? error.message : 'Failed to add song'
                    });
                }
                return;
            }

            // PING/PONG existed in the protocol but nothing ever answered a
            // PING, so guests had no way to tell a live channel from a dead
            // one. Answer before the generic command path, since PING is a
            // liveness probe rather than a state mutation.
            if (msg && msg.type === 'PING') {
                const pong: HostBroadcast = { type: 'PONG', serverTime: Date.now() };
                try {
                    conn.send(pong);
                } catch (e) {
                    console.warn('[PeerHost] Failed to answer PING:', e);
                }
                return;
            }

            if (!connectionsRef.current.has(conn.peer)) {
                sendWaitingApproval(conn.peer, conn);
                return;
            }

            if (isClientCommand(data)) {
                console.log('[PeerHost] Received command:', data);
                try {
                    const actorName = getClientName(conn.peer, data.type === 'ADD_SONG' || data.type === 'PLAYLIST_ADD' ? data.addedBy : undefined);
                    if (
                        isSingerOnlyTransportCommand(data) &&
                        !canPeerReorderQueue(conn.peer) &&
                        !(await canControlCurrentSong(actorName))
                    ) {
                        conn.send({
                            type: 'ERROR',
                            code: 'NOT_AUTHORIZED',
                            message: 'Solo el cantante o un Vice-KJ puede controlar esta cancion',
                        } satisfies HostBroadcast);
                        return;
                    }
                    if (isQueueReorderCommand(data) && !canPeerReorderQueue(conn.peer)) {
                        conn.send({
                            type: 'ERROR',
                            code: 'NOT_AUTHORIZED',
                            message: 'El KJ no permite cambiar el orden de la cola',
                        } satisfies HostBroadcast);
                        return;
                    }
                    // Process command in Rust backend
                    await processCommand(withTrustedActor(data, actorName));
                    // State update will be broadcast via Tauri event
                } catch (error) {
                    console.error('[PeerHost] Command processing failed:', error);
                    const errorMsg: HostBroadcast = {
                        type: 'ERROR',
                        code: 'COMMAND_FAILED',
                        message: typeof error === 'string' ? error : error instanceof Error ? error.message : 'Unknown error',
                    };
                    conn.send(errorMsg);
                }
            }
        });

        conn.on('close', () => {
            console.log('[PeerHost] Connection closed:', conn.peer);
            dropPeer(conn.peer);
        });

        // Without this, a guest whose phone slept or briefly dropped Wi-Fi —
        // the normal case at a party — stayed in the connection map forever.
        // Every subsequent broadcast then tried to write to a dead channel and
        // the client count was permanently wrong.
        conn.on('error', (err) => {
            console.warn('[PeerHost] Connection error, dropping peer:', conn.peer, err);
            dropPeer(conn.peer);
        });
    };

    const dropPeer = (peerId: string) => {
        dropConnection(peerId);
        dropPendingConnection(peerId);
    };

    const dropConnection = (peerId: string) => {
        clientNamesRef.current.delete(peerId);
        clientKeysRef.current.delete(peerId);
        setReorderAllowedPeerIds((prev) => {
            if (!prev.has(peerId)) return prev;
            const next = new Set(prev);
            next.delete(peerId);
            return next;
        });
        setClientNames((prev) => {
            if (!prev.has(peerId)) return prev;
            const next = new Map(prev);
            next.delete(peerId);
            return next;
        });
        setConnections((prev) => {
            if (!prev.has(peerId)) return prev;
            const next = new Map(prev);
            next.delete(peerId);
            return next;
        });
    };

    const dropPendingConnection = (peerId: string) => {
        pendingClientNamesRef.current.delete(peerId);
        pendingClientKeysRef.current.delete(peerId);
        setPendingClientNames((prev) => {
            if (!prev.has(peerId)) return prev;
            const next = new Map(prev);
            next.delete(peerId);
            return next;
        });
        setPendingConnections((prev) => {
            if (!prev.has(peerId)) return prev;
            const next = new Map(prev);
            next.delete(peerId);
            return next;
        });
    };

    const ensurePendingClientName = (peerId: string) => {
        if (pendingClientNamesRef.current.has(peerId)) return;
        pendingClientNamesRef.current.set(peerId, 'Guest');
        setPendingClientNames((prev) => {
            if (prev.has(peerId)) return prev;
            const next = new Map(prev);
            next.set(peerId, 'Guest');
            return next;
        });
    };

    const rememberClientName = (peerId: string, name: string) => {
        const clean = name.trim();
        if (clean) {
            clientNamesRef.current.set(peerId, clean);
            setClientNames((prev) => {
                const next = new Map(prev);
                next.set(peerId, clean);
                return next;
            });
        }
    };

    const rememberPendingClientName = (peerId: string, name: string) => {
        const clean = name.trim();
        if (clean) {
            pendingClientNamesRef.current.set(peerId, clean);
            setPendingClientNames((prev) => {
                const next = new Map(prev);
                next.set(peerId, clean);
                return next;
            });
        }
    };

    const rememberClientKey = (peerId: string, clientKey: string) => {
        if (!clientKey) return;
        clientKeysRef.current.set(peerId, clientKey);
        approvedClientKeysRef.current.add(clientKey);
    };

    const rememberPendingClientKey = (peerId: string, clientKey: string) => {
        if (!clientKey) return;
        pendingClientKeysRef.current.set(peerId, clientKey);
    };

    const sendWaitingApproval = (peerId: string, conn = pendingConnectionsRef.current.get(peerId)) => {
        if (!conn?.open) return;
        const name = pendingClientNamesRef.current.get(peerId) || 'Guest';
        const clientKey = pendingClientKeysRef.current.get(peerId);
        const previouslyKicked = kickedClientNamesRef.current.has(normalizeName(name)) ||
            (!!clientKey && blockedClientKeysRef.current.has(clientKey));
        conn.send({
            type: 'WAITING_APPROVAL',
            message: previouslyKicked
                ? 'En sala de espera, esperando confirmacion del KJ'
                : 'En sala de espera, esperando confirmacion del KJ',
        } satisfies HostBroadcast);
    };

    const approveClient = (peerId: string) => {
        const conn = pendingConnectionsRef.current.get(peerId);
        if (!conn?.open) {
            dropPendingConnection(peerId);
            return;
        }
        const name = pendingClientNamesRef.current.get(peerId) || 'Guest';
        const clientKey = pendingClientKeysRef.current.get(peerId);
        if (clientKey) {
            const previousPeerId = findPeerIdByClientKey(clientKey, peerId);
            if (previousPeerId) {
                const previousConn = connectionsRef.current.get(previousPeerId);
                try {
                    previousConn?.close();
                } catch (error) {
                    console.warn('[PeerHost] Failed to close previous client connection:', error);
                }
                dropConnection(previousPeerId);
            }
            approvedClientKeysRef.current.add(clientKey);
            blockedClientKeysRef.current.delete(clientKey);
            clientKeysRef.current.set(peerId, clientKey);
            if (reorderAllowedClientKeysRef.current.has(clientKey)) {
                setReorderAllowedPeerIds((prev) => new Set(prev).add(peerId));
            }
        }
        rememberClientName(peerId, name);
        setConnections((prev) => new Map(prev).set(peerId, conn));
        dropPendingConnection(peerId);
        setKickedClientNames((prev) => {
            const next = new Set(prev);
            next.delete(normalizeName(name));
            return next;
        });
        void sendStateUpdate(conn);
    };

    const rejectClient = (peerId: string) => {
        const name = pendingClientNamesRef.current.get(peerId) || 'Guest';
        const clientKey = pendingClientKeysRef.current.get(peerId);
        const conn = pendingConnectionsRef.current.get(peerId);
        if (clientKey) {
            blockedClientKeysRef.current.add(clientKey);
            approvedClientKeysRef.current.delete(clientKey);
            reorderAllowedClientKeysRef.current.delete(clientKey);
        }
        setReorderAllowedPeerIds((prev) => {
            if (!prev.has(peerId)) return prev;
            const next = new Set(prev);
            next.delete(peerId);
            return next;
        });
        setKickedClientNames((prev) => new Set(prev).add(normalizeName(name)));
        if (conn?.open) {
            conn.send({
                type: 'DISCONNECT',
                message: 'El KJ no ha aprobado tu entrada',
            } satisfies HostBroadcast);
            window.setTimeout(() => {
                try {
                    conn.close();
                } catch (error) {
                    console.warn('[PeerHost] Failed to close rejected client connection:', error);
                }
            }, 100);
        }
        dropPendingConnection(peerId);
    };

    const kickClient = (peerId: string) => {
        const conn = connectionsRef.current.get(peerId);
        const name = clientNamesRef.current.get(peerId) || 'Guest';
        const clientKey = clientKeysRef.current.get(peerId);
        if (clientKey) {
            blockedClientKeysRef.current.add(clientKey);
            approvedClientKeysRef.current.delete(clientKey);
            reorderAllowedClientKeysRef.current.delete(clientKey);
        }
        setReorderAllowedPeerIds((prev) => {
            if (!prev.has(peerId)) return prev;
            const next = new Set(prev);
            next.delete(peerId);
            return next;
        });
        setKickedClientNames((prev) => new Set(prev).add(normalizeName(name)));
        if (conn?.open) {
            conn.send({
                type: 'DISCONNECT',
                message: 'El KJ te ha desconectado de la sesion',
            } satisfies HostBroadcast);
            window.setTimeout(() => {
                try {
                    conn.close();
                } catch (error) {
                    console.warn('[PeerHost] Failed to close kicked client connection:', error);
                }
            }, 100);
        }
        dropConnection(peerId);
    };

    const getClientName = (peerId: string, fallback?: unknown) => {
        const known = clientNamesRef.current.get(peerId);
        if (known) return known;
        if (typeof fallback === 'string' && fallback.trim()) {
            rememberClientName(peerId, fallback);
            return fallback.trim();
        }
        return 'Guest';
    };

    const normalizeName = (name: string | undefined | null) => (name || '').trim().toLocaleLowerCase();

    const cleanClientKey = (value: unknown) =>
        typeof value === 'string' ? value.trim().slice(0, 128) : '';

    const findPeerIdByClientKey = (clientKey: string, exceptPeerId?: string) => {
        for (const [peerId, knownKey] of clientKeysRef.current.entries()) {
            if (peerId !== exceptPeerId && knownKey === clientKey) return peerId;
        }
        return undefined;
    };

    const canControlCurrentSong = async (actorName: string) => {
        const state = await getRoomState();
        const singer = state.player.currentSong?.addedBy;
        return !!singer && normalizeName(singer) === normalizeName(actorName);
    };

    function canPeerReorderQueue(peerId: string) {
        const clientKey = clientKeysRef.current.get(peerId);
        return reorderAllowedPeerIdsRef.current.has(peerId) ||
            (!!clientKey && reorderAllowedClientKeysRef.current.has(clientKey));
    }

    function sendSessionSettings(peerId: string, conn = connectionsRef.current.get(peerId)) {
        if (!conn?.open) return;
        conn.send({
            type: 'SESSION_SETTINGS',
            guestsCanInvite: guestsCanInviteRef.current,
            guestsCanReorderQueue: canPeerReorderQueue(peerId),
        } satisfies HostBroadcast);
    }

    const setClientCanReorderQueue = (peerId: string, value: boolean) => {
        const clientKey = clientKeysRef.current.get(peerId);
        if (clientKey) {
            if (value) {
                reorderAllowedClientKeysRef.current.add(clientKey);
            } else {
                reorderAllowedClientKeysRef.current.delete(clientKey);
            }
        }
        setReorderAllowedPeerIds((prev) => {
            const next = new Set(prev);
            if (value) {
                next.add(peerId);
            } else {
                next.delete(peerId);
            }
            return next;
        });
    };

    const isSingerOnlyTransportCommand = (command: ClientCommand) =>
        command.type === 'PLAY' || command.type === 'PAUSE' || command.type === 'SKIP';

    const isQueueReorderCommand = (command: ClientCommand) =>
        command.type === 'REORDER_QUEUE' ||
        command.type === 'MOVE_SONG_UP' ||
        command.type === 'MOVE_SONG_DOWN' ||
        command.type === 'MOVE_SONG_TO_TOP' ||
        command.type === 'MOVE_SONG_TO_BOTTOM';

    const withTrustedActor = (command: ClientCommand, actorName: string): ClientCommand => {
        if (command.type === 'ADD_SONG') {
            return { ...command, addedBy: actorName };
        }
        if (command.type === 'PLAYLIST_ADD') {
            return { ...command, addedBy: actorName };
        }
        return command;
    };

    const sendStateUpdate = async (conn: DataConnection) => {
        try {
            const state = await getRoomState();
            const publicState = {
                ...state,
                playlists: (state.playlists || []).filter(
                    (c: { visibility: string }) => c.visibility === 'public'
                ),
            };
            const broadcast: HostBroadcast = {
                type: 'STATE_UPDATE',
                state: publicState,
            };
            conn.send(broadcast);
            conn.send({
                type: 'SESSION_SETTINGS',
                guestsCanInvite: guestsCanInviteRef.current,
                guestsCanReorderQueue: canPeerReorderQueue(conn.peer),
            } satisfies HostBroadcast);
        } catch (error) {
            console.error('[PeerHost] Failed to send state update:', error);
        }
    };

    const broadcastToAll = (message: HostBroadcast) => {
        connections.forEach((conn) => {
            if (conn.open) {
                conn.send(message);
            }
        });
    };

    return {
        peer,
        connectionUrl,
        connectedClients: connections.size,
        connectedClientList: Array.from(connections.keys()).map((id) => ({
            id,
            displayName: clientNames.get(id) || 'Guest',
            canReorderQueue: canPeerReorderQueue(id),
        })),
        pendingClientList: Array.from(pendingConnections.keys()).map((id) => {
            const displayName = pendingClientNames.get(id) || 'Guest';
            const clientKey = pendingClientKeysRef.current.get(id);
            return {
                id,
                displayName,
                previouslyKicked: kickedClientNames.has(normalizeName(displayName)) ||
                    (!!clientKey && blockedClientKeysRef.current.has(clientKey)),
            };
        }),
        guestsCanInvite,
        setGuestsCanInvite,
        setClientCanReorderQueue,
        approveClient,
        rejectClient,
        kickClient,
        broadcastToAll,
    };
}


