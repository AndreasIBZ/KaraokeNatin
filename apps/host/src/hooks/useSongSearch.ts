import { useCallback, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SearchResult } from '@karaokenatin/shared';

export const FESTEJAR_SEARCH_KARAOKE_ONLY_KEY = 'festejar_search_karaoke_only';
const LEGACY_SEARCH_KEYS = ['karaoke_search_karaoke_only', 'library_resolve_karaoke_only'];

function readInitialKaraokeOnly() {
    const current = localStorage.getItem(FESTEJAR_SEARCH_KARAOKE_ONLY_KEY);
    if (current !== null) return current !== 'false';

    for (const key of LEGACY_SEARCH_KEYS) {
        const legacy = localStorage.getItem(key);
        if (legacy !== null) {
            localStorage.setItem(FESTEJAR_SEARCH_KARAOKE_ONLY_KEY, legacy === 'false' ? 'false' : 'true');
            return legacy !== 'false';
        }
    }

    localStorage.setItem(FESTEJAR_SEARCH_KARAOKE_ONLY_KEY, 'true');
    return true;
}

export function useSongSearch(limit = 10) {
    const [results, setResults] = useState<SearchResult[]>([]);
    const [loading, setLoading] = useState(false);
    const [karaokeOnly, setKaraokeOnlyState] = useState(readInitialKaraokeOnly);
    const requestRef = useRef(0);

    const setKaraokeOnly = useCallback((value: boolean) => {
        setKaraokeOnlyState(value);
        localStorage.setItem(FESTEJAR_SEARCH_KARAOKE_ONLY_KEY, value ? 'true' : 'false');
    }, []);

    const search = useCallback(async (query: string, options?: { limit?: number; karaokeOnly?: boolean }) => {
        const cleanQuery = query.trim();
        if (!cleanQuery) return [];

        const requestId = requestRef.current + 1;
        requestRef.current = requestId;
        setLoading(true);
        setResults([]);

        try {
            const nextResults = await invoke<SearchResult[]>('search_youtube', {
                query: cleanQuery,
                limit: options?.limit ?? limit,
                karaokeOnly: options?.karaokeOnly ?? karaokeOnly,
            });
            if (requestId === requestRef.current) {
                setResults(nextResults);
            }
            return nextResults;
        } finally {
            if (requestId === requestRef.current) {
                setLoading(false);
            }
        }
    }, [karaokeOnly, limit]);

    return {
        results,
        loading,
        karaokeOnly,
        setKaraokeOnly,
        search,
    };
}
