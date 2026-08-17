use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use parking_lot::RwLock;
use rusty_ytdl::search::{YouTube, SearchResult as YtSearchResult, SearchOptions, SearchType};
use std::time::Duration;
use tokio::time::timeout;

const YOUTUBE_VIDEOS_API_URL: &str = "https://www.googleapis.com/youtube/v3/videos";
const YOUTUBE_OEMBED_API_URL: &str = "https://www.youtube.com/oembed";
const YOUTUBE_EMBEDDABLE_CHECK_TIMEOUT_SECS: u64 = 10;
const YOUTUBE_OEMBED_CHECK_TIMEOUT_SECS: u64 = 5;
const YOUTUBE_VIDEOS_LIST_BATCH_SIZE: usize = 50;
const UNPLAYABLE_VIDEO_MESSAGE: &str = "Esta version no se puede reproducir. Elige otra.";

/// Search result from YouTube
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration: String,
    pub thumbnail: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct VideosListResponse {
    #[serde(default)]
    items: Vec<VideoStatusItem>,
}

#[derive(Debug, Deserialize)]
struct VideoStatusItem {
    id: String,
    status: VideoStatus,
}

#[derive(Debug, Deserialize)]
struct VideoStatus {
    #[serde(default)]
    embeddable: Option<bool>,
}

pub fn unplayable_video_message() -> String {
    UNPLAYABLE_VIDEO_MESSAGE.to_string()
}

fn youtube_api_key() -> Option<String> {
    std::env::var("YOUTUBE_API_KEY")
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

fn unique_video_ids(video_ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    video_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .filter_map(|id| {
            if seen.insert(id.to_string()) {
                Some(id.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn embeddable_ids_from_response(response: VideosListResponse) -> HashSet<String> {
    response
        .items
        .into_iter()
        .filter(|item| item.status.embeddable.unwrap_or(false))
        .map(|item| item.id)
        .collect()
}

pub fn filter_results_by_embeddable(
    results: Vec<SearchResult>,
    embeddable_ids: &HashSet<String>,
) -> Vec<SearchResult> {
    results
        .into_iter()
        .filter(|result| embeddable_ids.contains(&result.id))
        .collect()
}

async fn filter_results_by_embeddable_status(
    results: Vec<SearchResult>,
) -> Result<Vec<SearchResult>, String> {
    let video_ids = results.iter().map(|result| result.id.clone()).collect::<Vec<_>>();
    let embeddable_ids = fetch_embeddable_video_ids(&video_ids).await?;
    Ok(filter_results_by_embeddable(results, &embeddable_ids))
}

pub async fn fetch_embeddable_video_ids(video_ids: &[String]) -> Result<HashSet<String>, String> {
    let video_ids = unique_video_ids(video_ids);
    if video_ids.is_empty() {
        return Ok(HashSet::new());
    }

    let Some(api_key) = youtube_api_key() else {
        return fetch_embeddable_video_ids_with_oembed(&video_ids).await;
    };

    let client = reqwest::Client::new();
    let mut embeddable_ids = HashSet::new();

    for chunk in video_ids.chunks(YOUTUBE_VIDEOS_LIST_BATCH_SIZE) {
        let ids = chunk.join(",");
        let response = timeout(
            Duration::from_secs(YOUTUBE_EMBEDDABLE_CHECK_TIMEOUT_SECS),
            client
                .get(YOUTUBE_VIDEOS_API_URL)
                .query(&[
                    ("part", "status"),
                    ("id", ids.as_str()),
                    ("key", api_key.as_str()),
                ])
                .send(),
        )
        .await
        .map_err(|_| "YouTube embeddable check timed out".to_string())?
        .map_err(|e| format!("YouTube embeddable check failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("YouTube embeddable check failed: HTTP {}", status));
        }

        let parsed = timeout(
            Duration::from_secs(YOUTUBE_EMBEDDABLE_CHECK_TIMEOUT_SECS),
            response.json::<VideosListResponse>(),
        )
        .await
        .map_err(|_| "YouTube embeddable check response timed out".to_string())?
        .map_err(|e| format!("YouTube embeddable check returned invalid data: {}", e))?;

        embeddable_ids.extend(embeddable_ids_from_response(parsed));
    }

    Ok(embeddable_ids)
}

async fn fetch_embeddable_video_ids_with_oembed(video_ids: &[String]) -> Result<HashSet<String>, String> {
    let client = reqwest::Client::new();
    let mut embeddable_ids = HashSet::new();

    for video_id in video_ids {
        let video_url = format!("https://www.youtube.com/watch?v={}", video_id);
        let response = match timeout(
            Duration::from_secs(YOUTUBE_OEMBED_CHECK_TIMEOUT_SECS),
            client
                .get(YOUTUBE_OEMBED_API_URL)
                .query(&[
                    ("url", video_url.as_str()),
                    ("format", "json"),
                ])
                .send(),
        )
        .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => {
                log::warn!("[YouTube] Playable check failed for {}: {}", video_id, e);
                continue;
            }
            Err(_) => {
                log::warn!("[YouTube] Playable check timed out for {}", video_id);
                continue;
            }
        };

        if response.status().is_success() {
            embeddable_ids.insert(video_id.clone());
        } else {
            log::info!(
                "[YouTube] Filtering non-embeddable result {} via oEmbed HTTP {}",
                video_id,
                response.status()
            );
        }
    }

    Ok(embeddable_ids)
}

pub async fn ensure_video_is_embeddable(video_id: &str) -> Result<(), String> {
    let video_ids = vec![video_id.to_string()];
    let embeddable_ids = fetch_embeddable_video_ids(&video_ids).await?;
    if embeddable_ids.contains(video_id) {
        Ok(())
    } else {
        Err(unplayable_video_message())
    }
}

/// Cache TTL in seconds (30 minutes)
const CACHE_TTL_SECS: u64 = 1800;

/// Hard cap on the number of cached queries. Without this the cache grows for
/// the lifetime of the process — on a long-running TV host that's an
/// unbounded memory leak, since the TTL alone only hides stale entries from
/// readers, it never frees them.
const MAX_CACHE_ENTRIES: usize = 100;

/// Cache entry with timestamp for potential TTL
#[derive(Debug, Clone)]
struct CacheEntry {
    results: Vec<SearchResult>,
    cached_at: std::time::Instant,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.age_exceeds(Duration::from_secs(CACHE_TTL_SECS))
    }

    /// Same check as `is_expired`, but against an explicit threshold instead
    /// of the fixed TTL constant. Exists so the eviction/purge logic below is
    /// unit-testable without a real 30-minute wait.
    fn age_exceeds(&self, ttl: Duration) -> bool {
        self.cached_at.elapsed() > ttl
    }
}

/// Thread-safe in-memory cache for search results
static SEARCH_CACHE: LazyLock<RwLock<HashMap<String, CacheEntry>>> = LazyLock::new(|| {
    RwLock::new(HashMap::new())
});

/// Drop every entry older than `ttl`. Called on every cache write (i.e. every
/// "touch") rather than only when a specific stale key happens to be
/// re-queried, so a query that stops being repeated doesn't linger in memory
/// forever just because nobody asks for it again.
fn purge_older_than(cache: &mut HashMap<String, CacheEntry>, ttl: Duration) {
    cache.retain(|_, entry| !entry.age_exceeds(ttl));
}

/// Evict the single oldest entry once the cache is at capacity.
///
/// This is deliberately simple oldest-by-insertion-time eviction rather than
/// a full LRU (which would need extra bookkeeping — e.g. an access-order
/// list — to track *reads*, not just writes). Karaoke search traffic doesn't
/// need that precision, and plain "drop the oldest" is a lot easier to
/// verify correct.
fn evict_oldest_if_at_capacity(cache: &mut HashMap<String, CacheEntry>) {
    if cache.len() < MAX_CACHE_ENTRIES {
        return;
    }
    if let Some(oldest_key) = cache
        .iter()
        .min_by_key(|(_, entry)| entry.cached_at)
        .map(|(key, _)| key.clone())
    {
        cache.remove(&oldest_key);
    }
}

/// Look up a fresh (non-expired) cached result set.
fn get_cached(key: &str) -> Option<Vec<SearchResult>> {
    let cache = SEARCH_CACHE.read();
    cache
        .get(key)
        .filter(|entry| !entry.is_expired())
        .map(|entry| entry.results.clone())
}

/// Store a result set, purging expired entries and enforcing the size cap
/// first. Every store is a "touch" of the cache, which is when the purge
/// happens.
fn store_cached(key: String, results: Vec<SearchResult>) {
    let mut cache = SEARCH_CACHE.write();
    purge_older_than(&mut cache, Duration::from_secs(CACHE_TTL_SECS));
    evict_oldest_if_at_capacity(&mut cache);
    cache.insert(
        key,
        CacheEntry {
            results,
            cached_at: std::time::Instant::now(),
        },
    );
}

/// Search YouTube for videos matching the query using rusty_ytdl (pure Rust, no sidecar)
pub async fn search_youtube(
    query: &str,
    limit: u32,
    karaoke_only: bool,
) -> Result<Vec<SearchResult>, String> {
    let search_query = if karaoke_only {
        format!("{} karaoke", query)
    } else {
        query.to_string()
    };
    let mode = if karaoke_only { "karaoke" } else { "all" };
    let cache_key = format!("{}:{}:{}", mode, search_query.to_lowercase(), limit);

    // Check cache first
    if let Some(results) = get_cached(&cache_key) {
        log::info!("[YouTube] Cache hit for: {}", search_query);
        let filtered_results = filter_results_by_embeddable_status(results).await?;
        store_cached(cache_key, filtered_results.clone());
        return Ok(filtered_results);
    }

    log::info!("[YouTube] Cache miss, searching with rusty_ytdl for: {}", search_query);

    let youtube = YouTube::new().map_err(|e| format!("Failed to create YouTube client: {}", e))?;

    let raw_limit = limit.saturating_mul(3).clamp(limit, 30);
    let search_options = SearchOptions {
        limit: raw_limit as u64,
        search_type: SearchType::Video,
        safe_search: false,
    };

    let search_results = timeout(
        Duration::from_secs(10),
        youtube.search(&search_query, Some(&search_options))
    )
    .await
    .map_err(|_| "YouTube search timed out after 10 seconds".to_string())?
    .map_err(|e| format!("YouTube search failed: {}", e))?;

    let mut results = Vec::new();

    for item in search_results {
        match item {
            YtSearchResult::Video(video) => {
                let id = video.id.clone();
                let title = video.title.clone();
                let channel = video.channel.name.clone();

                let duration = format_duration_ms(video.duration);

                let thumbnail = video
                    .thumbnails
                    .last()
                    .map(|t| t.url.clone())
                    .unwrap_or_else(|| {
                        format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", id)
                    });

                let url = format!("https://youtu.be/{}", id);

                results.push(SearchResult {
                    id,
                    title,
                    channel,
                    duration,
                    thumbnail,
                    url,
                });
            }
            // Skip non-video results (playlists, channels)
            _ => continue,
        }
    }

    let mut results = filter_results_by_embeddable_status(results).await?;
    results.truncate(limit as usize);

    log::info!(
        "[YouTube] Found {} playable results via rusty_ytdl, caching...",
        results.len()
    );

    store_cached(cache_key, results.clone());

    Ok(results)
}

/// Format duration from milliseconds to MM:SS or HH:MM:SS
fn format_duration_ms(ms: u64) -> String {
    let total_secs = ms / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{}:{:02}", minutes, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_format_duration_ms() {
        assert_eq!(format_duration_ms(65000), "1:05");
        assert_eq!(format_duration_ms(3661000), "1:01:01");
        assert_eq!(format_duration_ms(0), "0:00");
    }

    fn dummy_result(id: &str) -> SearchResult {
        SearchResult {
            id: id.to_string(),
            title: format!("title-{id}"),
            channel: "channel".to_string(),
            duration: "1:00".to_string(),
            thumbnail: "".to_string(),
            url: "".to_string(),
        }
    }

    #[test]
    fn test_filter_results_keeps_embeddable_videos() {
        let mut embeddable_ids = HashSet::new();
        embeddable_ids.insert("ok-video".to_string());

        let filtered = filter_results_by_embeddable(
            vec![dummy_result("ok-video"), dummy_result("blocked-video")],
            &embeddable_ids,
        );

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "ok-video");
    }

    #[test]
    fn test_filter_results_removes_non_embeddable_videos() {
        let embeddable_ids = HashSet::new();

        let filtered = filter_results_by_embeddable(vec![dummy_result("blocked-video")], &embeddable_ids);

        assert!(filtered.is_empty());
    }

    #[test]
    fn test_filter_results_removes_missing_video_ids() {
        let mut embeddable_ids = HashSet::new();
        embeddable_ids.insert("other-video".to_string());

        let filtered = filter_results_by_embeddable(vec![dummy_result("missing-video")], &embeddable_ids);

        assert!(filtered.is_empty());
    }

    #[test]
    fn test_embeddable_ids_from_response_only_accepts_true_status() {
        let embeddable_ids = embeddable_ids_from_response(VideosListResponse {
            items: vec![
                VideoStatusItem {
                    id: "ok-video".to_string(),
                    status: VideoStatus {
                        embeddable: Some(true),
                    },
                },
                VideoStatusItem {
                    id: "blocked-video".to_string(),
                    status: VideoStatus {
                        embeddable: Some(false),
                    },
                },
                VideoStatusItem {
                    id: "unknown-video".to_string(),
                    status: VideoStatus { embeddable: None },
                },
            ],
        });

        assert!(embeddable_ids.contains("ok-video"));
        assert!(!embeddable_ids.contains("blocked-video"));
        assert!(!embeddable_ids.contains("unknown-video"));
    }

    #[test]
    fn test_cache_entry_age_exceeds_threshold() {
        let entry = CacheEntry {
            results: Vec::new(),
            cached_at: Instant::now(),
        };
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            entry.age_exceeds(Duration::from_millis(1)),
            "entry should be considered aged past a 1ms threshold after a 20ms sleep"
        );
        assert!(
            !entry.age_exceeds(Duration::from_secs(60)),
            "entry should not be considered aged past a 60s threshold"
        );
    }

    #[test]
    fn test_purge_older_than_removes_only_expired_entries() {
        let mut cache = HashMap::new();
        cache.insert(
            "old".to_string(),
            CacheEntry {
                results: vec![dummy_result("old")],
                cached_at: Instant::now(),
            },
        );
        std::thread::sleep(Duration::from_millis(25));
        cache.insert(
            "new".to_string(),
            CacheEntry {
                results: vec![dummy_result("new")],
                cached_at: Instant::now(),
            },
        );

        // Threshold chosen so the entry inserted ~25ms earlier counts as
        // expired but the one inserted immediately before this call does not.
        purge_older_than(&mut cache, Duration::from_millis(10));

        assert!(!cache.contains_key("old"), "expired entry should have been purged");
        assert!(cache.contains_key("new"), "fresh entry should survive the purge");
    }

    #[test]
    fn test_evict_oldest_if_at_capacity_enforces_cap() {
        let mut cache = HashMap::new();
        for i in 0..(MAX_CACHE_ENTRIES + 10) {
            evict_oldest_if_at_capacity(&mut cache);
            cache.insert(
                format!("key-{i}"),
                CacheEntry {
                    results: Vec::new(),
                    cached_at: Instant::now(),
                },
            );
        }
        assert_eq!(
            cache.len(),
            MAX_CACHE_ENTRIES,
            "cache should never grow past MAX_CACHE_ENTRIES"
        );
    }

    /// Exercises the real `SEARCH_CACHE` global through `store_cached`/
    /// `get_cached`, the same path `search_youtube` uses. This is the only
    /// test in the module that touches the shared static, so it doesn't need
    /// isolation from sibling tests.
    #[test]
    fn test_store_and_get_cached_enforces_cap_and_keeps_fresh_entries() {
        {
            let mut cache = SEARCH_CACHE.write();
            cache.clear();
        }

        assert!(get_cached("does-not-exist").is_none());

        store_cached("fresh-key".to_string(), vec![dummy_result("fresh")]);
        let fetched = get_cached("fresh-key");
        assert!(
            fetched.is_some(),
            "a freshly stored entry must be immediately retrievable"
        );
        assert_eq!(fetched.unwrap()[0].id, "fresh");

        for i in 0..(MAX_CACHE_ENTRIES + 20) {
            store_cached(format!("bulk-{i}"), Vec::new());
        }
        let len = SEARCH_CACHE.read().len();
        assert!(len <= MAX_CACHE_ENTRIES, "cache exceeded its cap: {len} entries");

        // Leave the shared cache clean for any other test that might run
        // later in this process.
        SEARCH_CACHE.write().clear();
    }
}
