use notify::{Event, EventKind as EventType, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const WATCHER_DEBOUNCE_MS: u64 = 400;
const INTERNAL_REFRESH_SUPPRESS_MS: u64 = 1_500;
const DB_UPDATED_EVENT: &str = "addons-db-updated";
const WATCH_ERROR_EVENT: &str = "addons-watch-error";

#[derive(Serialize, Clone)]
struct AddonsDbUpdatedEvent {
    data: crate::commands::Database,
    external: bool,
}

#[derive(Serialize, Clone)]
struct AddonsWatchErrorEvent {
    message: String,
}

enum WatcherSignal {
    Event,
    Error(String),
}

#[derive(Default)]
struct RefreshQueueState {
    running: bool,
    pending: bool,
    pending_external: bool,
}

#[derive(Default)]
struct RefreshCoordinator {
    queue: Mutex<RefreshQueueState>,
    suppress_until: Mutex<Option<Instant>>,
}

impl RefreshCoordinator {
    fn suppress_internal_refresh(&self) {
        if let Ok(mut guard) = self.suppress_until.lock() {
            *guard = Some(Instant::now() + Duration::from_millis(INTERNAL_REFRESH_SUPPRESS_MS));
        }
    }

    fn is_suppressed(&self) -> bool {
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

    fn request_refresh(self: &Arc<Self>, app_handle: AppHandle, external: bool) {
        let Ok(mut queue) = self.queue.lock() else {
            emit_watch_error(&app_handle, "Failed to acquire watcher refresh queue");
            return;
        };

        if queue.running {
            queue.pending = true;
            queue.pending_external |= external;
            return;
        }

        queue.running = true;
        drop(queue);

        spawn_refresh_loop(app_handle, self.clone(), external);
    }
}

pub struct AddonWatcherController {
    watcher: Option<RecommendedWatcher>,
    watched_paths: Vec<PathBuf>,
    coordinator: Arc<RefreshCoordinator>,
}

impl Default for AddonWatcherController {
    fn default() -> Self {
        Self {
            watcher: None,
            watched_paths: Vec::new(),
            coordinator: Arc::new(RefreshCoordinator::default()),
        }
    }
}

impl AddonWatcherController {
    pub fn suppress_internal_refresh(&self) {
        self.coordinator.suppress_internal_refresh();
    }

    pub fn rebind(&mut self, app_handle: &AppHandle, loading_dir: &Path) -> Result<(), String> {
        self.watcher = None;
        self.watched_paths.clear();

        let watched_paths = watched_paths_from_loading_dir(loading_dir);
        for path in &watched_paths {
            fs::create_dir_all(path).map_err(|err| {
                format!(
                    "Failed to create watched directory {}: {}",
                    path.display(),
                    err
                )
            })?;
        }

        let (tx, rx) = mpsc::channel::<WatcherSignal>();
        let app_handle_for_thread = app_handle.clone();
        let coordinator = self.coordinator.clone();
        std::thread::spawn(move || run_event_loop(rx, app_handle_for_thread, coordinator));

        let watched_paths_for_callback = watched_paths.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: Result<Event, notify::Error>| match result {
                Ok(event) => {
                    if is_relevant_event(&event, &watched_paths_for_callback) {
                        let _ = tx.send(WatcherSignal::Event);
                    }
                }
                Err(err) => {
                    let _ = tx.send(WatcherSignal::Error(err.to_string()));
                }
            })
            .map_err(|err| format!("Failed to create file watcher: {}", err))?;

        for path in &watched_paths {
            watcher
                .watch(path, RecursiveMode::NonRecursive)
                .map_err(|err| format!("Failed to watch {}: {}", path.display(), err))?;
        }

        self.watched_paths = watched_paths;
        self.watcher = Some(watcher);
        Ok(())
    }
}

fn run_event_loop(
    rx: Receiver<WatcherSignal>,
    app_handle: AppHandle,
    coordinator: Arc<RefreshCoordinator>,
) {
    loop {
        let signal = match rx.recv() {
            Ok(signal) => signal,
            Err(_) => return,
        };

        match signal {
            WatcherSignal::Error(message) => emit_watch_error(&app_handle, &message),
            WatcherSignal::Event => {
                loop {
                    match rx.recv_timeout(Duration::from_millis(WATCHER_DEBOUNCE_MS)) {
                        Ok(WatcherSignal::Event) => continue,
                        Ok(WatcherSignal::Error(message)) => {
                            emit_watch_error(&app_handle, &message)
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }

                let external = !coordinator.is_suppressed();
                coordinator.request_refresh(app_handle.clone(), external);
            }
        }
    }
}

fn spawn_refresh_loop(
    app_handle: AppHandle,
    coordinator: Arc<RefreshCoordinator>,
    initial_external: bool,
) {
    tauri::async_runtime::spawn(async move {
        let mut next_external = initial_external;

        loop {
            match crate::commands::handlers::rescan_database_snapshot(&app_handle).await {
                Ok((data, changed)) => {
                    if changed {
                        let _ = app_handle.emit(
                            DB_UPDATED_EVENT,
                            AddonsDbUpdatedEvent {
                                data,
                                external: next_external,
                            },
                        );
                    }
                }
                Err(message) => emit_watch_error(&app_handle, &message),
            }

            let Ok(mut queue) = coordinator.queue.lock() else {
                emit_watch_error(&app_handle, "Failed to finalize watcher refresh queue");
                return;
            };

            if queue.pending {
                next_external = queue.pending_external;
                queue.pending = false;
                queue.pending_external = false;
                drop(queue);
                continue;
            }

            queue.running = false;
            break;
        }
    });
}

pub fn rebind_addon_watcher(app_handle: &AppHandle, loading_dir: &Path) -> Result<(), String> {
    let state = app_handle.state::<crate::AppState>();
    let mut controller = state
        .addon_watcher
        .lock()
        .map_err(|_| "Failed to acquire addon watcher controller".to_string())?;
    controller.rebind(app_handle, loading_dir)
}

pub fn suppress_internal_refresh(state: &crate::AppState) {
    if let Ok(controller) = state.addon_watcher.lock() {
        controller.suppress_internal_refresh();
    }
}

pub fn emit_watch_error(app_handle: &AppHandle, message: &str) {
    log::warn!("addon watcher: {}", message);
    let _ = app_handle.emit(
        WATCH_ERROR_EVENT,
        AddonsWatchErrorEvent {
            message: message.to_string(),
        },
    );
}

fn is_relevant_event(event: &Event, watched_paths: &[PathBuf]) -> bool {
    match event.kind {
        EventType::Create(_) | EventType::Modify(_) | EventType::Remove(_) | EventType::Any => {}
        _ => return false,
    }

    event
        .paths
        .iter()
        .any(|path| is_relevant_addon_path(path, watched_paths))
}

fn is_relevant_addon_path(path: &Path, watched_paths: &[PathBuf]) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    if !(file_name.ends_with(".vpk") || file_name.ends_with(".vpk.disabled")) {
        return false;
    }

    watched_paths.iter().any(|watched| {
        path.parent()
            .map(|parent| parent == watched)
            .unwrap_or(false)
    })
}

fn watched_paths_from_loading_dir(loading_dir: &Path) -> Vec<PathBuf> {
    vec![loading_dir.to_path_buf(), loading_dir.join("workshop")]
}

#[cfg(test)]
mod tests {
    use super::{
        is_relevant_addon_path, watched_paths_from_loading_dir, AddonWatcherController,
        INTERNAL_REFRESH_SUPPRESS_MS,
    };
    use notify::{event::ModifyKind, Event, EventKind as EventType};
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn watched_paths_match_loading_and_workshop_roots() {
        let loading_dir = Path::new("/tmp/l4a-loading");
        let paths = watched_paths_from_loading_dir(loading_dir);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/tmp/l4a-loading"),
                PathBuf::from("/tmp/l4a-loading/workshop"),
            ]
        );
    }

    #[test]
    fn addon_path_filter_only_accepts_direct_vpk_entries() {
        let watched = watched_paths_from_loading_dir(Path::new("/tmp/l4a-loading"));

        assert!(is_relevant_addon_path(
            Path::new("/tmp/l4a-loading/test.vpk"),
            &watched
        ));
        assert!(is_relevant_addon_path(
            Path::new("/tmp/l4a-loading/workshop/test.vpk.disabled"),
            &watched
        ));
        assert!(!is_relevant_addon_path(
            Path::new("/tmp/l4a-loading/workshop/subdir/test.vpk"),
            &watched
        ));
        assert!(!is_relevant_addon_path(
            Path::new("/tmp/l4a-loading/readme.txt"),
            &watched
        ));

        let event = Event {
            kind: EventType::Modify(ModifyKind::Name(notify::event::RenameMode::Both)),
            paths: vec![PathBuf::from("/tmp/l4a-loading/test.vpk")],
            attrs: Default::default(),
        };
        assert!(super::is_relevant_event(&event, &watched));
    }

    #[test]
    fn suppression_window_marks_internal_refreshes_temporarily() {
        let controller = AddonWatcherController::default();
        assert!(!controller.coordinator.is_suppressed());
        controller.suppress_internal_refresh();
        assert!(controller.coordinator.is_suppressed());
        thread::sleep(Duration::from_millis(INTERNAL_REFRESH_SUPPRESS_MS + 100));
        assert!(!controller.coordinator.is_suppressed());
    }
}
