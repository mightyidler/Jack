use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const FAILURE_TTL: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct IconCache {
    entries: Mutex<VecDeque<(String, Arc<Entry>)>>,
}

#[derive(Default)]
struct Entry {
    value: Mutex<Option<CachedIcon>>,
    bytes: AtomicUsize,
}
struct CachedIcon {
    modified: Option<SystemTime>,
    attempted: Instant,
    icon: Option<String>,
}

impl IconCache {
    pub fn get_or_extract(
        &self,
        path: &str,
        extract: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        self.get_or_extract_with_metadata(
            path,
            || {
                std::fs::metadata(path)
                    .ok()
                    .and_then(|value| value.modified().ok())
            },
            extract,
        )
    }

    fn get_or_extract_with_metadata(
        &self,
        path: &str,
        metadata: impl FnOnce() -> Option<SystemTime>,
        extract: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        let key = path.to_lowercase();
        let entry = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries
                .iter()
                .position(|(saved, _)| saved == &key)
                .and_then(|index| entries.remove(index))
                .map(|(_, entry)| entry)
                .unwrap_or_default();
            entries.push_back((key, entry.clone()));
            while entries.len() > MAX_ENTRIES {
                entries.pop_front();
            }
            entry
        };
        // Only callers for this executable wait for Shell extraction and PNG encoding.
        let mut cached = entry.value.lock().unwrap();
        let modified = metadata();
        if let Some(value) = cached.as_ref()
            && value.modified == modified
            && (value.icon.is_some() || value.attempted.elapsed() < FAILURE_TTL)
        {
            return value.icon.clone();
        }
        let icon = extract();
        entry
            .bytes
            .store(icon.as_ref().map_or(0, String::len), Ordering::Relaxed);
        *cached = Some(CachedIcon {
            modified,
            attempted: Instant::now(),
            icon: icon.clone(),
        });
        drop(cached);
        let mut entries = self.entries.lock().unwrap();
        let mut bytes: usize = entries
            .iter()
            .map(|(_, entry)| entry.bytes.load(Ordering::Relaxed))
            .sum();
        while bytes > MAX_BYTES {
            let Some((_, evicted)) = entries.pop_front() else {
                break;
            };
            bytes = bytes.saturating_sub(evicted.bytes.load(Ordering::Relaxed));
        }
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modified_executable_invalidates_cached_success_and_failure_without_file_io() {
        let cache = IconCache::default();
        let calls = AtomicUsize::new(0);
        let extract = || {
            calls.fetch_add(1, Ordering::Relaxed);
            Some("png".into())
        };
        let first = Some(SystemTime::UNIX_EPOCH);
        let changed = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1));
        assert!(
            cache
                .get_or_extract_with_metadata("same.exe", || first, || None)
                .is_none()
        );
        assert!(
            cache
                .get_or_extract_with_metadata("SAME.exe", || first, extract)
                .is_none()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(
            cache
                .get_or_extract_with_metadata("same.exe", || changed, extract)
                .is_some()
        );
        assert!(
            cache
                .get_or_extract_with_metadata("SAME.exe", || changed, extract)
                .is_some()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(
            cache
                .get_or_extract_with_metadata("same.exe", || first, extract)
                .is_some()
        );
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn simultaneous_requests_extract_once_and_failures_are_cached_briefly() {
        let cache = IconCache::default();
        let calls = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    assert_eq!(
                        cache
                            .get_or_extract_with_metadata(
                                "same.exe",
                                || None,
                                || {
                                    calls.fetch_add(1, Ordering::Relaxed);
                                    Some("png".into())
                                }
                            )
                            .as_deref(),
                        Some("png")
                    )
                });
            }
        });
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        for _ in 0..3 {
            assert!(
                cache
                    .get_or_extract_with_metadata(
                        "missing.exe",
                        || None,
                        || {
                            calls.fetch_add(1, Ordering::Relaxed);
                            None
                        }
                    )
                    .is_none()
            );
        }
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        let entry = cache.entries.lock().unwrap().back().unwrap().1.clone();
        entry.value.lock().unwrap().as_mut().unwrap().attempted = Instant::now() - FAILURE_TTL;
        assert!(
            cache
                .get_or_extract_with_metadata("missing.exe", || None, || Some("recovered".into()))
                .is_some()
        );
    }

    #[test]
    fn retained_cache_obeys_entry_and_byte_limits() {
        let cache = IconCache::default();
        for index in 0..MAX_ENTRIES + 10 {
            cache.get_or_extract_with_metadata(&format!("{index}.exe"), || None, || None);
        }
        assert_eq!(cache.entries.lock().unwrap().len(), MAX_ENTRIES);
        for index in 0..12 {
            cache.get_or_extract_with_metadata(
                &format!("large-{index}.exe"),
                || None,
                || Some("x".repeat(1024 * 1024)),
            );
        }
        let entries = cache.entries.lock().unwrap();
        assert!(
            entries
                .iter()
                .map(|(_, entry)| entry.bytes.load(Ordering::Relaxed))
                .sum::<usize>()
                <= MAX_BYTES
        );
        assert!(entries.len() <= MAX_ENTRIES);
    }
}
