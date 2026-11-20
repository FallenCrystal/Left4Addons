use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WATCHER_DEBOUNCE: Duration = Duration::from_millis(350);
const DEFAULT_SUPPRESS_WINDOW: Duration = Duration::from_millis(1500);

pub type WatchCallback = Arc<dyn Fn(bool) + Send + Sync>;
pub type WatchErrorCallback = Arc<dyn Fn(String) + Send + Sync>;

pub struct AddonWatcher {
    watcher: Option<RecommendedWatcher>,
    suppress_until: Arc<Mutex<Option<Instant>>>,
}

impl Default for AddonWatcher {
    fn default() -> Self {
        Self {
            watcher: None,
            suppress_until: Arc::new(Mutex::new(None)),
        }
    }
}

impl AddonWatcher {
    pub fn suppress_internal(&self) {
        self.suppress_for(DEFAULT_SUPPRESS_WINDOW);
    }

    pub fn suppress_for(&self, duration: Duration) {
        if let Ok(mut guard) = self.suppress_until.lock() {
            *guard = Some(Instant::now() + duration);
        }
    }

    pub fn is_suppressed(&self) -> bool {
        let Ok(mut guard) = self.suppress_until.lock() else {
            return false;
        };
        match *guard {
            Some(deadline) if Instant::now() < deadline => true,
            Some(_) => {
                *guard = None;
                false
            }
            None => false,
        }
    }

    pub fn watch(
        &mut self,
        loading_dir: &Path,
        workshop_dir: &Path,
        on_change: WatchCallback,
        on_error: WatchErrorCallback,
    ) -> Result<(), String> {
        self.watcher = None;
        let mut watched_paths = Vec::new();
        for path in [loading_dir, workshop_dir] {
            if !path.as_os_str().is_empty() {
                std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
                let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                if !watched_paths.contains(&canonical) {
                    watched_paths.push(canonical);
                }
            }
        }

        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let suppress = self.suppress_until.clone();
        std::thread::spawn(move || loop {
            match rx.recv() {
                Ok(Err(msg)) => on_error(msg),
                Ok(Ok(())) => {
                    loop {
                        match rx.recv_timeout(WATCHER_DEBOUNCE) {
                            Ok(Ok(())) => continue,
                            Ok(Err(msg)) => on_error(msg),
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    let suppressed = suppress
                        .lock()
                        .ok()
                        .and_then(|g| *g)
                        .is_some_and(|until| Instant::now() < until);
                    on_change(!suppressed);
                }
                Err(_) => return,
            }
        });

        let filter_paths = watched_paths.clone();
        let mut watcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    if is_relevant_event(&event, &filter_paths) {
                        let _ = tx.send(Ok(()));
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(err.to_string()));
                }
            })
            .map_err(|e| e.to_string())?;

        for path in &watched_paths {
            if !path.as_os_str().is_empty() && path.exists() {
                watcher
                    .watch(path, RecursiveMode::NonRecursive)
                    .map_err(|e| e.to_string())?;
            }
        }

        self.watcher = Some(watcher);
        Ok(())
    }
}

pub fn is_relevant_event(event: &Event, watched_paths: &[PathBuf]) -> bool {
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any => {}
        _ => return false,
    }
    event.paths.iter().any(|p| {
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            return false;
        };
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".vpk") || lower.ends_with(".vpk.disabled")) {
            return false;
        }
        let Some(parent) = p.parent() else {
            return false;
        };
        let canon_parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
        watched_paths
            .iter()
            .any(|w| w == parent || w == &canon_parent)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::ModifyKind;

    #[test]
    fn relevant_event_filters_only_direct_vpk_children() {
        let watched = vec![
            PathBuf::from("/tmp/loading"),
            PathBuf::from("/tmp/workshop"),
        ];
        let ok_ev = Event {
            kind: EventKind::Modify(ModifyKind::Any),
            paths: vec![PathBuf::from("/tmp/loading/abc.vpk.disabled")],
            attrs: Default::default(),
        };
        let nested_ev = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![PathBuf::from("/tmp/loading/sub/abc.vpk")],
            attrs: Default::default(),
        };
        assert!(is_relevant_event(&ok_ev, &watched));
        assert!(!is_relevant_event(&nested_ev, &watched));
    }

    #[test]
    fn suppression_window_expires() {
        let watcher = AddonWatcher::default();
        assert!(!watcher.is_suppressed());
        watcher.suppress_for(Duration::from_millis(40));
        assert!(watcher.is_suppressed());
        std::thread::sleep(Duration::from_millis(60));
        assert!(!watcher.is_suppressed());
    }
}
