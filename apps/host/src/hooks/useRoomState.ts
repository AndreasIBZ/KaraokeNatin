import { useState, useEffect, useRef, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { RoomState, Song, PlaylistCollection } from '@karaokenatin/shared';
import { createRoom, getRoomState } from '../lib/commands';

// Re-export types for components
export type { Song, RoomState, PlaylistCollection };

// Global ref to track if any input is focused (prevents re-renders while typing)
let _isInputFocused = false;
let _pendingState: RoomState | null = null;

export function setHostInputFocused(focused: boolean) {
    _isInputFocused = focused;
    // When unfocusing, flush any pending state
    if (!focused && _pendingState && _flushCallback) {
        _flushCallback(_pendingState);
        _pendingState = null;
    }
}

let _flushCallback: ((state: RoomState) => void) | null = null;

/**
 * Hook to manage room state from Rust backend
 */
export function useRoomState() {
    const [roomState, setRoomState] = useState<RoomState | null>(null);
    const [loading, setLoading] = useState(true);
    const roomStateRef = useRef<RoomState | null>(null);

    const applyRoomState = useCallback((state: RoomState) => {
        roomStateRef.current = state;
        setRoomState(state);
    }, []);

    // Register the flush callback
    useEffect(() => {
        _flushCallback = applyRoomState;
        return () => { _flushCallback = null; };
    }, [applyRoomState]);

    const initializeRoom = async () => {
        try {
            // Create room in Rust backend
            await createRoom();
            // Fetch initial state
            await refreshRoomState();
        } catch (error) {
            console.error('[useRoomState] Failed to initialize room:', error);
        } finally {
            setLoading(false);
        }
    };

    const refreshRoomState = useCallback(async () => {
        const state = await getRoomState();
        applyRoomState(state);
        setLoading(false);
    }, [applyRoomState]);

    useEffect(() => {
        // Subscribe to room state updates from Rust
        const unlisten = listen<RoomState>('room_state_updated', (event) => {
            if (_isInputFocused && !hasTransportChange(roomStateRef.current, event.payload)) {
                // Defer update to avoid re-rendering while user is typing
                _pendingState = event.payload;
            } else {
                applyRoomState(event.payload);
            }
        });

        return () => {
            unlisten.then((fn) => fn());
        };
    }, []);

    return { roomState, loading, initializeRoom, refreshRoomState };
}

function hasTransportChange(previous: RoomState | null, next: RoomState) {
    if (!previous) return true;

    const prevPlayer = previous.player;
    const nextPlayer = next.player;

    return (
        prevPlayer.currentSong?.id !== nextPlayer.currentSong?.id ||
        prevPlayer.status !== nextPlayer.status ||
        prevPlayer.volume !== nextPlayer.volume ||
        prevPlayer.isMuted !== nextPlayer.isMuted ||
        Math.abs(prevPlayer.currentTime - nextPlayer.currentTime) > 2.5 ||
        Math.abs(prevPlayer.duration - nextPlayer.duration) > 0.5
    );
}
