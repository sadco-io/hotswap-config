//! File watching for automatic configuration reloads.

use crate::error::{ConfigError, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
struct WatchedEntry {
    /// Canonical parent joined with leaf file name (or canonical dir if `is_dir`).
    target_path: PathBuf,
    /// Resolved canonical target if `target_path` is a symlink (e.g. Kubernetes ConfigMap `..data/...`).
    resolved_target: Option<PathBuf>,
    /// Directory registered with `notify` (`NonRecursive`).
    watch_dir: PathBuf,
    /// Whether the target itself is a watched directory.
    is_dir: bool,
}

#[derive(Default, Debug)]
struct WatchState {
    entries: Vec<WatchedEntry>,
}

impl WatchState {
    fn matches_event_path(&self, ev_path: &Path) -> bool {
        self.entries.iter().any(|entry| {
            if entry.is_dir {
                ev_path == entry.watch_dir || ev_path.parent() == Some(entry.watch_dir.as_path())
            } else if ev_path == entry.target_path
                || entry.resolved_target.as_deref() == Some(ev_path)
                || ev_path == entry.watch_dir.join("..data")
            {
                true
            } else if (ev_path == entry.watch_dir
                || ev_path.parent() == Some(entry.watch_dir.as_path()))
                && entry.resolved_target.is_some()
            {
                // On kqueue/inotify, kubelet's atomic writer creates a new timestamped dir and
                // swaps `..data_tmp` -> `..data` inside `watch_dir`; trigger if the symlink's
                // canonical target changed!
                let current_resolved = entry
                    .target_path
                    .canonicalize()
                    .ok()
                    .filter(|r| r != &entry.target_path);
                current_resolved != entry.resolved_target
            } else {
                false
            }
        })
    }
}

/// Resolve a file or directory path so its parent directory is canonicalized
/// (important on macOS where `/var` -> `/private/var`), while preserving the
/// leaf file name so atomic rename saves and symlink swaps are observed.
fn resolve_watch_entry(path: &Path, require_exists: bool) -> Result<WatchedEntry> {
    if require_exists {
        let meta = std::fs::metadata(path)
            .map_err(|e| ConfigError::LoadError(format!("Failed to resolve path: {}", e)))?;
        let symlink_meta = std::fs::symlink_metadata(path)
            .map_err(|e| ConfigError::LoadError(format!("Failed to resolve path: {}", e)))?;

        if meta.is_dir() && !symlink_meta.file_type().is_symlink() {
            let canonical_dir = path
                .canonicalize()
                .map_err(|e| ConfigError::LoadError(format!("Failed to resolve path: {}", e)))?;
            return Ok(WatchedEntry {
                target_path: canonical_dir.clone(),
                resolved_target: None,
                watch_dir: canonical_dir,
                is_dir: true,
            });
        }
    } else if let Ok(symlink_meta) = std::fs::symlink_metadata(path) {
        if symlink_meta.is_dir() && !symlink_meta.file_type().is_symlink() {
            let canonical_dir = path.canonicalize().map_err(|e| {
                ConfigError::LoadError(format!("Failed to resolve path for unwatching: {}", e))
            })?;
            return Ok(WatchedEntry {
                target_path: canonical_dir.clone(),
                resolved_target: None,
                watch_dir: canonical_dir,
                is_dir: true,
            });
        }
    }

    let file_name = path.file_name().ok_or_else(|| {
        ConfigError::LoadError(format!(
            "Failed to resolve file name for path: {}",
            path.display()
        ))
    })?;

    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };

    let canonical_parent = parent.canonicalize().map_err(|e| {
        ConfigError::LoadError(format!("Failed to resolve parent directory: {}", e))
    })?;

    let target_path = canonical_parent.join(file_name);
    let resolved_target = target_path
        .canonicalize()
        .ok()
        .filter(|resolved| resolved != &target_path);

    Ok(WatchedEntry {
        target_path,
        resolved_target,
        watch_dir: canonical_parent,
        is_dir: false,
    })
}

/// Register watches on a `WatchedEntry` (parent directory + direct file/symlink target for kqueue).
fn register_entry_watches(
    watcher: &mut RecommendedWatcher,
    entry: &WatchedEntry,
    watch_dir_needed: bool,
) -> Result<()> {
    if watch_dir_needed {
        watcher
            .watch(&entry.watch_dir, RecursiveMode::NonRecursive)
            .map_err(|e| ConfigError::WatchError(format!("Failed to watch directory: {}", e)))?;
    }

    if !entry.is_dir {
        watcher
            .watch(&entry.target_path, RecursiveMode::NonRecursive)
            .map_err(|e| ConfigError::WatchError(format!("Failed to watch path: {}", e)))?;

        if let Some(resolved) = &entry.resolved_target {
            let _ = watcher.watch(resolved, RecursiveMode::NonRecursive);
        }

        let data_link = entry.watch_dir.join("..data");
        if std::fs::symlink_metadata(&data_link).is_ok() {
            let _ = watcher.watch(&data_link, RecursiveMode::NonRecursive);
        }
    }

    Ok(())
}

/// Configuration watcher that monitors files for changes.
///
/// Uses the `notify` crate to watch configuration files (both directly and via their
/// parent directories so atomic editor saves and symlink swaps are detected across
/// `inotify`, `kqueue`, and Windows) and trigger reloads when they change.
/// Includes single-task coalescing debounce to avoid rapid reloads.
///
/// # Examples
///
/// ```rust,no_run
/// use hotswap_config::notify::ConfigWatcher;
/// use std::time::Duration;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let (watcher, mut rx) = ConfigWatcher::new(Duration::from_millis(500))?;
/// watcher.watch("/path/to/config.yaml").await?;
///
/// // Listen for reload signals
/// while let Some(()) = rx.recv().await {
///     println!("Config file changed, reload triggered!");
/// }
/// # Ok(())
/// # }
/// ```
pub struct ConfigWatcher {
    watcher: Arc<Mutex<RecommendedWatcher>>,
    debounce_duration: Duration,
    state: Arc<Mutex<WatchState>>,
}

impl fmt::Debug for ConfigWatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let paths: Vec<PathBuf> = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .iter()
            .map(|e| e.target_path.clone())
            .collect();
        f.debug_struct("ConfigWatcher")
            .field("debounce_duration", &self.debounce_duration)
            .field("watched_paths", &paths)
            .finish()
    }
}

impl ConfigWatcher {
    /// Create a new configuration watcher.
    ///
    /// # Arguments
    ///
    /// * `debounce_duration` - Minimum coalescing window for reload triggers (default: 500ms)
    ///
    /// # Returns
    ///
    /// Returns a tuple of `(ConfigWatcher, mpsc::Receiver<()>)`. The receiver yields `()`
    /// whenever a reload should be triggered, and closes cleanly when `ConfigWatcher` drops.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::WatchError` if the underlying file watcher cannot be created.
    pub fn new(debounce_duration: Duration) -> Result<(Self, mpsc::Receiver<()>)> {
        let (tx, rx) = mpsc::channel(100);
        let debounce = debounce_duration;

        // Channel for matching raw events from notify
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<Event>();
        let state = Arc::new(Mutex::new(WatchState::default()));
        let state_for_cb = Arc::clone(&state);

        // Create the notify watcher
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                if matches!(
                    event.kind,
                    notify::EventKind::Modify(_)
                        | notify::EventKind::Create(_)
                        | notify::EventKind::Remove(_)
                ) {
                    let matched = {
                        let guard = state_for_cb.lock().unwrap_or_else(|e| e.into_inner());
                        event
                            .paths
                            .iter()
                            .any(|ev_path| guard.matches_event_path(ev_path))
                    };
                    if matched {
                        let _ = event_tx.send(event);
                    }
                }
            }
        })
        .map_err(|e| ConfigError::WatchError(format!("Failed to create file watcher: {}", e)))?;

        let watcher_arc = Arc::new(Mutex::new(watcher));
        let watcher_for_task = Arc::downgrade(&watcher_arc);
        let state_for_task = Arc::clone(&state);

        // Spawn a single coalescing debounce task (no per-event task storm)
        tokio::spawn(async move {
            while let Some(_first_event) = event_rx.recv().await {
                let deadline = tokio::time::Instant::now() + debounce;

                loop {
                    match tokio::time::timeout_at(deadline, event_rx.recv()).await {
                        Err(_elapsed) => {
                            while event_rx.try_recv().is_ok() {}
                            break;
                        }
                        Ok(Some(_event)) => {}
                        Ok(None) => return,
                    }
                }

                // Re-attach file/symlink vnode watches in case an atomic rename or
                // symlink swap replaced the underlying inode (essential on kqueue).
                if let Some(watcher_mutex) = watcher_for_task.upgrade() {
                    let refreshed_targets: Vec<(PathBuf, Option<PathBuf>, PathBuf)> = {
                        let mut guard = state_for_task.lock().unwrap_or_else(|e| e.into_inner());
                        let mut targets = Vec::new();
                        for entry in &mut guard.entries {
                            if !entry.is_dir {
                                entry.resolved_target = entry
                                    .target_path
                                    .canonicalize()
                                    .ok()
                                    .filter(|r| r != &entry.target_path);
                                targets.push((
                                    entry.target_path.clone(),
                                    entry.resolved_target.clone(),
                                    entry.watch_dir.join("..data"),
                                ));
                            }
                        }
                        targets
                    };

                    let mut w = watcher_mutex.lock().unwrap_or_else(|e| e.into_inner());
                    for (target_path, resolved_target, data_link) in refreshed_targets {
                        if target_path.exists() {
                            let _ = w.watch(&target_path, RecursiveMode::NonRecursive);
                        }
                        if let Some(resolved) = resolved_target {
                            if resolved.exists() {
                                let _ = w.watch(&resolved, RecursiveMode::NonRecursive);
                            }
                        }
                        if std::fs::symlink_metadata(&data_link).is_ok() {
                            let _ = w.watch(&data_link, RecursiveMode::NonRecursive);
                        }
                    }
                }

                if tx.send(()).await.is_err() {
                    break;
                }
            }
        });

        Ok((
            Self {
                watcher: watcher_arc,
                debounce_duration,
                state,
            },
            rx,
        ))
    }

    /// Add a path to watch for changes.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file or directory to watch
    ///
    /// # Errors
    ///
    /// Returns `ConfigError::LoadError` if the path does not exist, or
    /// `ConfigError::WatchError` if the OS watcher fails to register.
    pub async fn watch(&self, path: impl AsRef<Path>) -> Result<()> {
        let entry = resolve_watch_entry(path.as_ref(), true)?;

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let dir_already_watched = state.entries.iter().any(|e| e.watch_dir == entry.watch_dir);

        {
            let mut watcher = self.watcher.lock().unwrap_or_else(|e| e.into_inner());
            register_entry_watches(&mut watcher, &entry, !dir_already_watched)?;
        }

        if !state
            .entries
            .iter()
            .any(|e| e.target_path == entry.target_path)
        {
            state.entries.push(entry);
        }

        Ok(())
    }

    /// Stop watching a specific path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to stop watching
    ///
    /// # Errors
    ///
    /// Returns an error if the path cannot be resolved or unwatched.
    pub async fn unwatch(&self, path: impl AsRef<Path>) -> Result<()> {
        let query = resolve_watch_entry(path.as_ref(), false)?;

        let (removed_entry, should_unwatch_dir) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let pos = state
                .entries
                .iter()
                .position(|e| e.target_path == query.target_path)
                .ok_or_else(|| {
                    ConfigError::WatchError(format!(
                        "Path is not currently watched: {}",
                        path.as_ref().display()
                    ))
                })?;
            let removed = state.entries.remove(pos);
            let unwatch_dir = !state
                .entries
                .iter()
                .any(|e| e.watch_dir == removed.watch_dir);
            (removed, unwatch_dir)
        };

        let mut watcher = self.watcher.lock().unwrap_or_else(|e| e.into_inner());
        if !removed_entry.is_dir {
            let _ = watcher.unwatch(&removed_entry.target_path);
            if let Some(resolved) = &removed_entry.resolved_target {
                let _ = watcher.unwatch(resolved);
            }
        }

        if should_unwatch_dir {
            watcher
                .unwatch(&removed_entry.watch_dir)
                .map_err(|e| ConfigError::WatchError(format!("Failed to unwatch path: {}", e)))?;
        }

        Ok(())
    }

    /// Get the debounce duration for this watcher.
    pub fn debounce_duration(&self) -> Duration {
        self.debounce_duration
    }

    /// Get a list of currently watched target paths.
    pub async fn watched_paths(&self) -> Vec<PathBuf> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .iter()
            .map(|e| e.target_path.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_watcher_creation() {
        let result = ConfigWatcher::new(Duration::from_millis(100));
        assert!(result.is_ok());
        let (watcher, _rx) = result.unwrap();
        assert!(format!("{:?}", watcher).contains("ConfigWatcher"));
    }

    #[tokio::test]
    async fn test_watch_file() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");
        fs::write(&config_path, "port: 8080").unwrap();

        let (watcher, _rx) = ConfigWatcher::new(Duration::from_millis(100)).unwrap();
        let result = watcher.watch(&config_path).await;
        assert!(result.is_ok());

        let paths = watcher.watched_paths().await;
        assert_eq!(paths.len(), 1);
    }

    #[tokio::test]
    async fn test_watch_nonexistent_file() {
        let (watcher, _rx) = ConfigWatcher::new(Duration::from_millis(100)).unwrap();
        let result = watcher.watch("/nonexistent/config.yaml").await;
        assert!(matches!(result, Err(ConfigError::LoadError(_))));
    }

    #[tokio::test]
    async fn test_file_change_triggers_reload() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");
        fs::write(&config_path, "port: 8080").unwrap();

        let (watcher, mut rx) = ConfigWatcher::new(Duration::from_millis(100)).unwrap();
        watcher.watch(&config_path).await.unwrap();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            fs::write(&config_path, "port: 9090").unwrap();
        });

        let result = timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_atomic_rename_and_subsequent_writes_trigger_reload() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");
        let tmp_path = temp_dir.path().join(".config.yaml.swp");
        fs::write(&config_path, "port: 8080").unwrap();

        let (watcher, mut rx) = ConfigWatcher::new(Duration::from_millis(80)).unwrap();
        watcher.watch(&config_path).await.unwrap();

        // 1. Atomic editor save #1 (write temp + rename over target)
        fs::write(&tmp_path, "port: 9090").unwrap();
        fs::rename(&tmp_path, &config_path).unwrap();

        let sig1 = timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(matches!(sig1, Ok(Some(()))));

        // 2. Atomic editor save #2
        fs::write(&tmp_path, "port: 9091").unwrap();
        fs::rename(&tmp_path, &config_path).unwrap();

        let sig2 = timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(matches!(sig2, Ok(Some(()))));

        // 3. Subsequent in-place write after atomic renames
        fs::write(&config_path, "port: 9092").unwrap();

        let sig3 = timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(matches!(sig3, Ok(Some(()))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_kubernetes_configmap_symlink_swap_triggers_reload() {
        use std::os::unix::fs::symlink;

        let temp_dir = TempDir::new().unwrap();
        let dir_v1 = temp_dir.path().join("..2026_11_14_v1");
        fs::create_dir(&dir_v1).unwrap();
        fs::write(dir_v1.join("config.yaml"), "port: 8080").unwrap();

        let data_link = temp_dir.path().join("..data");
        let config_link = temp_dir.path().join("config.yaml");
        symlink(&dir_v1, &data_link).unwrap();
        symlink(Path::new("..data").join("config.yaml"), &config_link).unwrap();

        let (watcher, mut rx) = ConfigWatcher::new(Duration::from_millis(80)).unwrap();
        watcher.watch(&config_link).await.unwrap();

        // Simulate kubelet atomic_writer.go ConfigMap projection update:
        // 1. Create new timestamped directory & write updated config
        // 2. Create `..data_tmp` -> `dir_v2` and atomically rename over `..data`
        // 3. Remove old timestamped directory `dir_v1`
        let dir_v2 = temp_dir.path().join("..2026_11_14_v2");
        fs::create_dir(&dir_v2).unwrap();
        fs::write(dir_v2.join("config.yaml"), "port: 9090").unwrap();
        let data_tmp = temp_dir.path().join("..data_tmp");
        symlink(&dir_v2, &data_tmp).unwrap();
        fs::rename(&data_tmp, &data_link).unwrap();
        fs::remove_dir_all(&dir_v1).unwrap();

        let sig = timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(matches!(sig, Ok(Some(()))));
    }

    #[tokio::test]
    async fn test_burst_events_coalesced_into_single_signal() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");
        fs::write(&config_path, "port: 8000").unwrap();

        let (watcher, mut rx) = ConfigWatcher::new(Duration::from_millis(200)).unwrap();
        watcher.watch(&config_path).await.unwrap();

        // Rapid burst of 20 writes within a few ms
        for i in 1..=20 {
            fs::write(&config_path, format!("port: {}", 8000 + i)).unwrap();
        }

        // First coalesced signal arrives
        let first = timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(matches!(first, Ok(Some(()))));

        // No second signal arrives once the burst window finishes
        let second = timeout(Duration::from_millis(300), rx.recv()).await;
        assert!(
            second.is_err(),
            "Burst should coalesce into a single reload"
        );
    }

    #[tokio::test]
    async fn test_drop_watcher_closes_channel_cleanly() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");
        fs::write(&config_path, "port: 8080").unwrap();

        let (watcher, mut rx) = ConfigWatcher::new(Duration::from_millis(100)).unwrap();
        watcher.watch(&config_path).await.unwrap();

        drop(watcher);

        // Channel closes promptly when ConfigWatcher is dropped
        let closed = timeout(Duration::from_millis(500), rx.recv()).await;
        assert_eq!(closed, Ok(None));
    }

    #[tokio::test]
    async fn test_unwatch_shared_parent_keeps_sibling_watched() {
        let temp_dir = TempDir::new().unwrap();
        let path_a = temp_dir.path().join("a.yaml");
        let path_b = temp_dir.path().join("b.yaml");
        fs::write(&path_a, "a: 1").unwrap();
        fs::write(&path_b, "b: 1").unwrap();

        let (watcher, mut rx) = ConfigWatcher::new(Duration::from_millis(80)).unwrap();
        watcher.watch(&path_a).await.unwrap();
        watcher.watch(&path_b).await.unwrap();
        assert_eq!(watcher.watched_paths().await.len(), 2);

        // Unwatch `a.yaml` — `b.yaml` in the same directory must stay watched!
        watcher.unwatch(&path_a).await.unwrap();
        assert_eq!(watcher.watched_paths().await.len(), 1);

        // Writing `a.yaml` should NOT trigger reload
        fs::write(&path_a, "a: 2").unwrap();
        let no_sig = timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(no_sig.is_err());

        // Writing `b.yaml` MUST still trigger reload
        fs::write(&path_b, "b: 2").unwrap();
        let sig = timeout(Duration::from_secs(2), rx.recv()).await;
        assert!(matches!(sig, Ok(Some(()))));

        // Unwatching `b.yaml` removes the directory watch too
        watcher.unwatch(&path_b).await.unwrap();
        assert!(watcher.watched_paths().await.is_empty());
    }

    #[tokio::test]
    async fn test_debounce_duration() {
        let duration = Duration::from_millis(500);
        let (watcher, _rx) = ConfigWatcher::new(duration).unwrap();
        assert_eq!(watcher.debounce_duration(), duration);
    }
}
