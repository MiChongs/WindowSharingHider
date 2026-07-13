use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::LazyLock;

use parking_lot::Mutex;

use crate::model::WindowIcon;

const MAX_CACHED_ICONS: usize = 256;

#[derive(Clone)]
struct CacheEntry {
    icon: Option<WindowIcon>,
    last_used: u64,
}

#[derive(Default)]
struct IconCache {
    entries: HashMap<String, CacheEntry>,
    clock: u64,
}

impl IconCache {
    fn get(&mut self, key: &str) -> Option<Option<WindowIcon>> {
        self.clock = self.clock.wrapping_add(1);
        let entry = self.entries.get_mut(key)?;
        entry.last_used = self.clock;
        Some(entry.icon.clone())
    }

    fn insert(&mut self, key: String, icon: Option<WindowIcon>) {
        self.clock = self.clock.wrapping_add(1);
        if self.entries.len() >= MAX_CACHED_ICONS && !self.entries.contains_key(&key) {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            key,
            CacheEntry {
                icon,
                last_used: self.clock,
            },
        );
    }
}

static ICON_CACHE: LazyLock<Mutex<IconCache>> = LazyLock::new(|| Mutex::new(IconCache::default()));

pub(crate) fn icon_for_process_path(path: &str) -> Option<WindowIcon> {
    if path.is_empty() {
        return None;
    }

    let key = normalize_path(path);
    if let Some(cached) = ICON_CACHE.lock().get(&key) {
        return cached;
    }

    let loaded = try_load_icon(path).or_else(|| try_load_icon(path));
    ICON_CACHE.lock().insert(key, loaded.clone());
    loaded
}

fn try_load_icon(path: &str) -> Option<WindowIcon> {
    catch_unwind(AssertUnwindSafe(|| {
        windows_icons::get_icon_by_path(Path::new(path))
            .ok()
            .and_then(|image| {
                let width = image.width();
                let height = image.height();
                WindowIcon::from_rgba(path.to_owned(), width, height, image.into_raw())
            })
    }))
    .unwrap_or(None)
}

fn normalize_path(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_icon(source: &str, value: u8) -> WindowIcon {
        WindowIcon::from_rgba(source.to_owned(), 1, 1, vec![value, value, value, 255])
            .expect("the test icon has one valid RGBA pixel")
    }

    #[test]
    fn current_executable_icon_is_cached_and_well_formed() {
        let path = std::env::current_exe().expect("the test executable path must be available");
        let path = path.to_string_lossy();
        let first = icon_for_process_path(&path).expect("Windows must expose the executable icon");
        let second = icon_for_process_path(&path).expect("the cached icon must remain available");

        assert_eq!(first, second);
        assert_eq!(first.source(), path);
        assert_eq!(
            first.rgba().len(),
            first.width() as usize * first.height() as usize * 4
        );
    }

    #[test]
    fn cache_is_bounded_and_evicts_the_least_recently_used_entry() {
        let mut cache = IconCache::default();
        for index in 0..MAX_CACHED_ICONS {
            let key = format!("process-{index}");
            cache.insert(key.clone(), Some(test_icon(&key, index as u8)));
        }
        assert!(cache.get("process-0").is_some());

        cache.insert("new-process".into(), Some(test_icon("new-process", 42)));

        assert_eq!(cache.entries.len(), MAX_CACHED_ICONS);
        assert!(cache.get("process-0").is_some());
        assert!(cache.get("process-1").is_none());
        assert!(cache.get("new-process").is_some());
    }

    #[test]
    fn empty_process_path_uses_the_ui_fallback() {
        assert!(icon_for_process_path("").is_none());
    }
}
