//! Watching the library folders, so files added, removed, renamed or
//! edited in them show up without a rescan.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Weak;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::db::Folder;
use crate::engine::Engine;
use crate::scan::is_audio;

/// Changes settle this long before a scan reads them...
const QUIET: Duration = Duration::from_secs(1);
/// ...though a stream of them, as from a long copy, is read this often.
const AT_MOST: Duration = Duration::from_secs(5);

pub struct Watcher {
    inner: RecommendedWatcher,
    watched: HashSet<String>,
}

impl Watcher {
    pub fn new(engine: Weak<Engine>) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let inner = notify::recommended_watcher(tx)?;
        std::thread::Builder::new()
            .name("watch".into())
            .spawn(move || settle(rx, engine))
            .map_err(notify::Error::io)?;
        Ok(Self {
            inner,
            watched: HashSet::new(),
        })
    }

    /// Watches `folders`, and only them.
    pub fn sync(&mut self, folders: &[Folder]) {
        let wanted: HashSet<String> = folders.iter().map(|f| f.path.clone()).collect();
        for path in self.watched.difference(&wanted) {
            let _ = self.inner.unwatch(Path::new(path));
        }
        self.watched.retain(|p| wanted.contains(p));
        for path in wanted {
            if self.watched.contains(&path) {
                continue;
            }
            match self.inner.watch(Path::new(&path), RecursiveMode::Recursive) {
                Ok(()) => {
                    self.watched.insert(path);
                }
                Err(e) => eprintln!("can't watch {path}: {e}"),
            }
        }
    }
}

/// Gathers changes until they settle, then has the engine scan where they
/// were.
fn settle(rx: mpsc::Receiver<notify::Result<Event>>, engine: Weak<Engine>) {
    while let Ok(first) = rx.recv() {
        let mut paths = HashSet::new();
        let mut rescan = false;
        let mut take = |event: notify::Result<Event>| match event {
            Ok(e) if matches!(e.kind, EventKind::Access(_)) => {}
            Ok(e) => {
                rescan |= e.need_rescan();
                paths.extend(e.paths.into_iter().filter(|p| worth_reading(p)));
            }
            Err(e) => eprintln!("watching failed: {e}"),
        };
        take(first);
        let start = Instant::now();
        while let Some(left) = AT_MOST.checked_sub(start.elapsed()) {
            match rx.recv_timeout(left.min(QUIET)) {
                Ok(event) => take(event),
                Err(RecvTimeoutError::Timeout) if left > QUIET => break,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if paths.is_empty() && !rescan {
            continue;
        }
        let Some(engine) = engine.upgrade() else {
            return;
        };
        engine.files_changed(paths.into_iter().collect(), rescan);
    }
}

/// Whether a change at `path` could change the library: hidden files are
/// never read, and other files only when they're audio.
fn worth_reading(path: &Path) -> bool {
    let hidden = path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with('.'));
    !hidden && (is_audio(path) || !path.is_file())
}
