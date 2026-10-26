//! The engine: the library in memory, kept in step with its folders, and
//! everything the app asks of it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::Serialize;

use crate::analysis::Descriptor;
use crate::clap::Embedding;
use crate::db::{self, Chosen, Folder, Stored};
use crate::library::{self, Guess, Info, Library, Query, Ranked, Row, View};
use crate::scan::{self, Roots};
use crate::watch::Watcher;
use crate::{decode, waveform};

const THUMB_BYTES: usize = waveform::THUMB_COLUMNS * waveform::BYTES_PER_COLUMN;
/// Searches kept to page through.
const CACHED: usize = 8;
/// Progress goes out at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Analyzing,
    /// CLAP listening to what the analysis found.
    Listening,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub stage: Stage,
    pub done: usize,
    pub total: usize,
}

/// The library's folders, categories and counts.
#[derive(Serialize)]
pub struct Overview<'a> {
    #[serde(flatten)]
    pub info: Info<'a>,
    pub version: u64,
}

/// What the engine says unasked. The overview is big with a big library,
/// so it only goes out when it changes. A scan's progress goes on its own.
#[derive(Serialize)]
#[serde(tag = "event", content = "data", rename_all = "camelCase")]
pub enum Event<'a> {
    Overview(Overview<'a>),
    /// How far the scan has got, or None once it's done.
    Progress(Option<Progress>),
}

#[derive(Serialize)]
pub struct SearchResult {
    pub total: usize,
    pub rows: Vec<Row>,
}

/// A sound from outside the library, dropped in to find ones like it. Each
/// gets the next negative id, so the page never mixes two of them up.
struct Guest {
    id: i64,
    path: String,
    descriptor: Descriptor,
    waveform: Vec<u8>,
}

/// The favorites, which the engine keeps beside the library rather than in
/// it, so a rebuild can never undo one.
#[derive(Default)]
struct Favorites {
    ids: HashSet<i64>,
    /// Counts the changes, for searches that filter by them.
    generation: u64,
}

/// A search kept to page through, and what it was made from.
struct Cached {
    query: Query,
    version: u64,
    guest: Option<i64>,
    favorites: Option<u64>,
    view: Arc<View>,
    ranked: Arc<Ranked>,
}

struct Events {
    /// When the progress last went out.
    last: Instant,
    /// Whether the progress has moved since.
    moved: bool,
}

pub struct Engine {
    db_path: PathBuf,
    /// CLAP's model, to listen with. Without it, guesses go by the analysis
    /// alone.
    model: Option<PathBuf>,
    /// For requests, opened as they're needed and kept for the next one.
    /// Scans open their own.
    conns: Mutex<Vec<Connection>>,
    view: Mutex<Arc<View>>,
    /// Held while a new view is made from the current one, so changes land
    /// one after another.
    publishing: Mutex<()>,
    /// Held while the folders change, so each change publishes the folders
    /// it made.
    refoldering: Mutex<()>,
    favorites: Mutex<Arc<Favorites>>,
    /// Favorites change one at a time, so the set ends as the database does.
    favoriting: Mutex<()>,
    /// So do chosen categories.
    choosing: Mutex<()>,
    guest: Mutex<Option<Arc<Guest>>>,
    /// Drops land one at a time, so the guest is the last sound dropped.
    dropping: Mutex<()>,
    cache: Mutex<Vec<Cached>>,
    progress: Mutex<Option<Progress>>,
    /// Scans asked for since the last one started, as one.
    scans: Mutex<Option<Roots>>,
    scans_waiting: Condvar,
    /// The library to guess categories for next.
    to_guess: Mutex<Option<Arc<Library>>>,
    to_guess_waiting: Condvar,
    /// Set when a newer library cancels the guessing under way.
    superseded: AtomicBool,
    events: Mutex<Events>,
    progress_moved: Condvar,
    send: Box<dyn Fn(&Event) + Send + Sync>,
    watcher: Mutex<Option<Watcher>>,
}

/// Locks `m` even if a request panicked while holding it: nothing here is
/// left half-changed by one.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait<'a, T>(cv: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    cv.wait(guard).unwrap_or_else(PoisonError::into_inner)
}

fn err(e: impl ToString) -> String {
    e.to_string()
}

pub(crate) fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .expect("starting a thread");
}

impl Engine {
    /// Loads the library kept at `db_path`, to listen to with the CLAP
    /// model at `model`. `send` gets each event to pass on.
    pub fn open(
        db_path: &Path,
        model: Option<&Path>,
        send: impl Fn(&Event) + Send + Sync + 'static,
    ) -> Result<Arc<Self>, String> {
        let conn = db::open(db_path).map_err(err)?;
        let library = Library::new(
            db::folders(&conn).map_err(err)?,
            db::samples(&conn).map_err(err)?,
        );
        let favorites = Favorites {
            ids: db::favorites(&conn).map_err(err)?,
            generation: 0,
        };
        let unguessed = vec![Guess::default(); library.len()];
        Ok(Arc::new(Self {
            db_path: db_path.to_path_buf(),
            model: model.map(Path::to_path_buf),
            conns: Mutex::new(vec![conn]),
            view: Mutex::new(Arc::new(View::new(Arc::new(library), unguessed, 1))),
            publishing: Mutex::new(()),
            refoldering: Mutex::new(()),
            favorites: Mutex::new(Arc::new(favorites)),
            favoriting: Mutex::new(()),
            choosing: Mutex::new(()),
            guest: Mutex::new(None),
            dropping: Mutex::new(()),
            cache: Mutex::new(Vec::new()),
            progress: Mutex::new(None),
            scans: Mutex::new(None),
            scans_waiting: Condvar::new(),
            to_guess: Mutex::new(None),
            to_guess_waiting: Condvar::new(),
            superseded: AtomicBool::new(false),
            events: Mutex::new(Events {
                last: Instant::now(),
                moved: false,
            }),
            progress_moved: Condvar::new(),
            send: Box::new(send),
            watcher: Mutex::new(None),
        }))
    }

    /// Loads the library, says what's in it, and keeps it up to date: scans
    /// to catch up, watches its folders, listens and guesses categories.
    pub fn start(
        db_path: &Path,
        model: Option<&Path>,
        send: impl Fn(&Event) + Send + Sync + 'static,
    ) -> Result<Arc<Self>, String> {
        let engine = Self::open(db_path, model, send)?;
        match Watcher::new(Arc::downgrade(&engine)) {
            Ok(w) => *lock(&engine.watcher) = Some(w),
            Err(e) => eprintln!("can't watch the library's folders: {e}"),
        }
        let view = engine.view();
        engine.watch(&view.library.folders);
        let e = engine.clone();
        spawn("scan", move || e.scan_forever());
        let e = engine.clone();
        spawn("guess", move || e.guess_forever());
        let e = engine.clone();
        spawn("progress", move || e.send_progress_forever());
        engine.changed();
        engine.guess(view.library.clone());
        // Catch up on whatever changed while the app was closed.
        engine.queue_scan(Roots::All);
        Ok(engine)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn view(&self) -> Arc<View> {
        lock(&self.view).clone()
    }

    fn favorites(&self) -> Arc<Favorites> {
        lock(&self.favorites).clone()
    }

    /// Runs `f` on a connection of its own.
    fn with_conn<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> Result<T, String> {
        let pooled = lock(&self.conns).pop();
        let mut conn = match pooled {
            Some(c) => c,
            None => db::open(&self.db_path).map_err(err)?,
        };
        let result = f(&mut conn).map_err(err);
        lock(&self.conns).push(conn);
        result
    }

    fn set_view(&self, view: View) {
        *lock(&self.view) = Arc::new(view);
        // Searches of older views can't be asked for again.
        lock(&self.cache).clear();
    }

    /// Makes the library `change` of the current one.
    fn publish_change(&self, change: impl FnOnce(&Library) -> Library) {
        let _publishing = lock(&self.publishing);
        let current = self.view();
        let library = Arc::new(change(&current.library));
        self.set_view(current.carry(library.clone()));
        // Still publishing, so the guesses asked for last are for the
        // library published last.
        self.guess(library);
    }

    /// Makes the library the current one plus `upserts`, less `removed`,
    /// with its folders now `folders` if they changed.
    pub fn publish(&self, upserts: Vec<Stored>, removed: &[i64], folders: Option<Vec<Folder>>) {
        let removed: HashSet<i64> = removed.iter().copied().collect();
        self.publish_change(|library| {
            let folders = folders.unwrap_or_else(|| library.folders.clone());
            library.with_changes(folders, upserts, &removed)
        });
        self.changed();
    }

    /// Publishes what a scan stored a moment ago, less what left with a
    /// folder since: no folder can change meanwhile.
    pub fn publish_stored(&self, upserts: Vec<Stored>, failed: &[i64]) -> Result<(), String> {
        let _refoldering = lock(&self.refoldering);
        let ids: Vec<i64> = upserts.iter().map(|s| s.id).collect();
        let kept = self.with_conn(|conn| db::existing(conn, &ids))?;
        let upserts = upserts
            .into_iter()
            .filter(|s| kept.contains(&s.id))
            .collect();
        self.publish(upserts, failed, None);
        Ok(())
    }

    /// Makes the library the current one with what CLAP heard.
    pub fn publish_heard(&self, heard: Vec<(i64, Embedding)>) {
        // What's shown changes once the guesses do.
        self.publish_change(|library| library.with_embeddings(heard));
    }

    pub fn has_folder(&self, id: i64) -> bool {
        self.view().library.folder(id).is_some()
    }

    pub fn set_progress(&self, progress: Option<Progress>) {
        let was = std::mem::replace(&mut *lock(&self.progress), progress);
        match progress {
            Some(_) => {
                lock(&self.events).moved = true;
                self.progress_moved.notify_one();
            }
            // The last word on a scan always goes out.
            None if was.is_some() => self.send_progress(&mut lock(&self.events)),
            None => {}
        }
    }

    /// Sends the overview now: something in it changed. Events are made and
    /// sent under the events lock, so they go out in the order they were
    /// made.
    fn changed(&self) {
        let _events = lock(&self.events);
        let view = self.view();
        let favorites = self.favorites();
        (self.send)(&Event::Overview(Overview {
            info: view.info(&favorites.ids),
            version: view.version,
        }));
    }

    fn send_progress(&self, events: &mut Events) {
        (self.send)(&Event::Progress(*lock(&self.progress)));
        events.last = Instant::now();
        events.moved = false;
    }

    /// Sends the progress as it moves, though not too often.
    fn send_progress_forever(&self) {
        let mut events = lock(&self.events);
        loop {
            if !events.moved {
                events = wait(&self.progress_moved, events);
                continue;
            }
            let due = events.last + PROGRESS_EVERY;
            let now = Instant::now();
            if now < due {
                events = self
                    .progress_moved
                    .wait_timeout(events, due - now)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                continue;
            }
            self.send_progress(&mut events);
        }
    }

    pub fn queue_scan(&self, roots: Roots) {
        let mut scans = lock(&self.scans);
        *scans = Some(match scans.take() {
            Some(pending) => pending.merge(roots),
            None => roots,
        });
        self.scans_waiting.notify_one();
    }

    /// Whether a scan is waiting to run.
    pub fn scan_waiting(&self) -> bool {
        lock(&self.scans).is_some()
    }

    /// Runs the scans asked for, one at a time, on threads of their own,
    /// and listens to what each found.
    fn scan_forever(&self) {
        let pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("analysis {i}"))
            .build()
            .expect("starting the analysis threads");
        loop {
            let roots = {
                let mut scans = lock(&self.scans);
                loop {
                    if let Some(roots) = scans.take() {
                        break roots;
                    }
                    scans = wait(&self.scans_waiting, scans);
                }
            };
            if let Err(e) = scan::run(self, roots, &pool) {
                eprintln!("scan failed: {e}");
            }
            if let Some(model) = &self.model
                && let Err(e) = scan::listen(self, model, &pool)
            {
                eprintln!("listening failed: {e}");
            }
            self.set_progress(None);
        }
    }

    /// Guesses categories for `library` in the background, giving up on the
    /// guessing under way.
    fn guess(&self, library: Arc<Library>) {
        let mut next = lock(&self.to_guess);
        *next = Some(library);
        self.superseded.store(true, Ordering::Relaxed);
        self.to_guess_waiting.notify_one();
    }

    fn guess_forever(&self) {
        let pool = rayon::ThreadPoolBuilder::new()
            .thread_name(|i| format!("guess {i}"))
            .build()
            .expect("starting the guessing threads");
        loop {
            let library = {
                let mut next = lock(&self.to_guess);
                loop {
                    if let Some(library) = next.take() {
                        self.superseded.store(false, Ordering::Relaxed);
                        break library;
                    }
                    next = wait(&self.to_guess_waiting, next);
                }
            };
            let guessed =
                pool.install(|| library.guess(|| self.superseded.load(Ordering::Relaxed)));
            if let Some(guessed) = guessed {
                self.apply_guesses(&library, guessed);
            }
        }
    }

    fn apply_guesses(&self, library: &Arc<Library>, guessed: Vec<Guess>) {
        {
            let _publishing = lock(&self.publishing);
            let current = self.view();
            if !Arc::ptr_eq(&current.library, library) {
                return;
            }
            match current.with_guesses(guessed) {
                Some(view) => self.set_view(view),
                None => return,
            }
        }
        self.changed();
    }

    fn watch(&self, folders: &[Folder]) {
        if let Some(w) = lock(&self.watcher).as_mut() {
            w.sync(folders);
        }
    }

    /// Scans where the watcher saw `paths` change, or everything if it lost
    /// track.
    pub fn files_changed(&self, paths: Vec<PathBuf>, rescan: bool) {
        if rescan {
            return self.queue_scan(Roots::All);
        }
        let view = self.view();
        let dirs: Vec<(i64, PathBuf)> = paths
            .into_iter()
            .filter_map(|p| {
                let path = p.to_str()?;
                let folder = view
                    .library
                    .folder_of(path)
                    .or_else(|| view.library.folders.iter().find(|f| f.path == path))?;
                let root = Path::new(&folder.path);
                let dir = if p.is_dir() { p.as_path() } else { p.parent()? };
                let dir = if dir.starts_with(root) { dir } else { root };
                Some((folder.id, dir.to_path_buf()))
            })
            .collect();
        if !dirs.is_empty() {
            self.queue_scan(Roots::Dirs(dirs));
        }
    }

    pub fn add_folders(&self, paths: &[String]) -> Result<(), String> {
        let folders = paths
            .iter()
            .map(|p| match std::fs::canonicalize(p) {
                Ok(dir) if dir.is_dir() => Ok(dir),
                Ok(_) => Err(format!("{p} isn't a folder")),
                Err(e) => Err(unreadable(p, e)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.add_folders_at(&folders)
    }

    fn add_folders_at(&self, paths: &[PathBuf]) -> Result<(), String> {
        let _refoldering = lock(&self.refoldering);
        let (added, folders) = self.with_conn(|conn| {
            let mut added = Vec::new();
            for p in paths {
                if let Some(id) = db::add_folder(conn, &p.to_string_lossy())? {
                    added.push((id, p.clone()));
                }
            }
            Ok((added, db::folders(conn)?))
        })?;
        if added.is_empty() {
            return Ok(());
        }
        self.publish(Vec::new(), &[], Some(folders.clone()));
        self.watch(&folders);
        self.queue_scan(Roots::Dirs(added));
        Ok(())
    }

    pub fn remove_folder(&self, id: i64) -> Result<(), String> {
        let _refoldering = lock(&self.refoldering);
        let folders = self.with_conn(|conn| {
            db::remove_folder(conn, id)?;
            db::folders(conn)
        })?;
        // A scan reading it passes over what's left of it.
        self.publish(Vec::new(), &[], Some(folders.clone()));
        self.watch(&folders);
        Ok(())
    }

    pub fn rescan(&self) {
        self.queue_scan(Roots::All);
    }

    /// Folders join the library. The first sound comes back as its row, a
    /// new guest's if it isn't in the library.
    pub fn drop_paths(&self, paths: &[String]) -> Result<Option<Row>, String> {
        let _dropping = lock(&self.dropping);
        let (folders, files): (Vec<PathBuf>, Vec<PathBuf>) = paths
            .iter()
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .partition(|p| p.is_dir());
        if !folders.is_empty() {
            self.add_folders_at(&folders)?;
        }
        let Some(file) = files.iter().find(|p| scan::is_audio(p)) else {
            return Ok(None);
        };
        let path = file.to_string_lossy().into_owned();
        let view = self.view();
        if let Some(i) = view.library.find_path(&path) {
            return Ok(Some(view.row(i, None, &self.favorites().ids)));
        }
        let analyzed = scan::analyze_file(file)?;
        let mut guest = lock(&self.guest);
        let id = guest.as_ref().map_or(-1, |g| g.id - 1);
        let row = library::guest_row(id, &path, &analyzed);
        *guest = Some(Arc::new(Guest {
            id,
            path,
            descriptor: analyzed.descriptor,
            waveform: analyzed.waveform,
        }));
        Ok(Some(row))
    }

    /// Rows `offset..offset + limit` of what matches `q`, and how many do.
    pub fn search(&self, q: &Query, offset: usize, limit: usize) -> SearchResult {
        let view = self.view();
        let favorites = self.favorites();
        let guest = lock(&self.guest).clone();
        let guest_id = q
            .similar
            .filter(|&id| id < 0)
            .map(|_| guest.as_ref().map_or(0, |g| g.id));
        let favorites_generation = q.favorites.then_some(favorites.generation);
        let hit = {
            let mut cache = lock(&self.cache);
            let found = cache.iter().position(|c| {
                c.version == view.version
                    && c.guest == guest_id
                    && c.favorites == favorites_generation
                    && c.query == *q
            });
            found.map(|i| {
                // Most recently used first.
                let c = cache.remove(i);
                let hit = (c.view.clone(), c.ranked.clone());
                cache.insert(0, c);
                hit
            })
        };
        let (view, ranked) = match hit {
            Some(hit) => hit,
            None => {
                let reference = guest
                    .as_ref()
                    .filter(|g| q.similar == Some(g.id))
                    .map(|g| &g.descriptor);
                let ranked = Arc::new(view.search(q, &favorites.ids, reference));
                let mut cache = lock(&self.cache);
                if lock(&self.view).version == view.version {
                    cache.insert(
                        0,
                        Cached {
                            query: q.clone(),
                            version: view.version,
                            guest: guest_id,
                            favorites: favorites_generation,
                            view: view.clone(),
                            ranked: ranked.clone(),
                        },
                    );
                    cache.truncate(CACHED);
                }
                (view, ranked)
            }
        };
        SearchResult {
            total: ranked.positions.len(),
            rows: view.rows(&ranked, offset, limit, &favorites.ids),
        }
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> Result<(), String> {
        {
            let _favoriting = lock(&self.favoriting);
            if !self.with_conn(|conn| db::set_favorite(conn, id, favorite))? {
                return Ok(());
            }
            let mut favorites = lock(&self.favorites);
            let mut ids = favorites.ids.clone();
            if favorite {
                ids.insert(id);
            } else {
                ids.remove(&id);
            }
            *favorites = Arc::new(Favorites {
                ids,
                generation: favorites.generation + 1,
            });
        }
        self.changed();
        Ok(())
    }

    /// Makes `chosen` the sample's category, over what its name says. With
    /// none, it's the name's again. Its row as it is now, if it's still here.
    pub fn set_category(&self, id: i64, chosen: Option<Chosen>) -> Result<Option<Row>, String> {
        {
            let _choosing = lock(&self.choosing);
            if !self.with_conn(|conn| db::set_chosen(conn, id, chosen.as_ref()))? {
                return Ok(None);
            }
            self.publish_change(|library| library.with_chosen(id, chosen));
        }
        self.changed();
        let view = self.view();
        Ok(view
            .library
            .position(id)
            .map(|i| view.row(i, None, &self.favorites().ids)))
    }

    /// Thumbnail waveforms for `ids`, back to back (blank for unknown ids).
    pub fn thumbnails(&self, ids: &[i64]) -> Result<Vec<u8>, String> {
        let guest = lock(&self.guest).clone();
        let mut out = vec![0u8; ids.len() * THUMB_BYTES];
        self.with_conn(|conn| {
            for (slot, &id) in out.as_chunks_mut::<THUMB_BYTES>().0.iter_mut().zip(ids) {
                let stored;
                let waveform = match guest.as_ref().filter(|g| g.id == id) {
                    Some(g) => &g.waveform,
                    None => match db::thumbnail(conn, id)? {
                        Some(w) => {
                            stored = w;
                            &stored
                        }
                        None => continue,
                    },
                };
                let n = waveform.len().min(THUMB_BYTES);
                slot[..n].copy_from_slice(&waveform[..n]);
            }
            Ok(())
        })?;
        Ok(out)
    }

    fn sample_path(&self, id: i64) -> Result<String, String> {
        if let Some(g) = lock(&self.guest).as_ref().filter(|g| g.id == id) {
            return Ok(g.path.clone());
        }
        self.view()
            .library
            .get(id)
            .map(|s| s.path.clone())
            .ok_or_else(|| "this sample has left the library".into())
    }

    /// A detailed waveform, `columns` wide.
    pub fn waveform(&self, id: i64, columns: usize) -> Result<Vec<u8>, String> {
        let d = scan::decode(Path::new(&self.sample_path(id)?))?;
        Ok(waveform::render(
            &d.mono,
            d.sample_rate,
            columns.clamp(16, 8192),
        ))
    }

    /// The sample's audio, for playing.
    pub fn pcm(&self, id: i64) -> Result<decode::Pcm, String> {
        scan::decode_channels(Path::new(&self.sample_path(id)?))
    }
}

fn unreadable(path: &str, e: std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => format!("{path} couldn't be found"),
        _ => format!("{path} couldn't be opened: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DESCRIPTOR_DIMS;
    use crate::db::{Analyzed, Scanned};
    use crate::library::Sort;

    fn analyzed(v: f32) -> Analyzed {
        Analyzed {
            duration: 1.0,
            sample_rate: 44100,
            channels: 1,
            bits: Some(16),
            pitch: None,
            key: None,
            tempo: None,
            looped: false,
            descriptor: Descriptor(vec![v; DESCRIPTOR_DIMS]),
            waveform: vec![1; THUMB_BYTES],
        }
    }

    fn query(favorites: bool) -> Query {
        Query {
            text: String::new(),
            folder: None,
            dir: None,
            category: None,
            subcategory: None,
            kind: None,
            favorites,
            similar: None,
            weights: [1.0; 4],
            sort: Sort::Name,
            descending: false,
            seed: 0,
        }
    }

    #[test]
    fn favorites_outlast_rebuilds() {
        let dir = std::env::temp_dir().join(format!("samplary-engine-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("library.sqlite");
        let mut conn = db::open(&db_path).unwrap();
        let folder = db::add_folder(&mut conn, "/lib/Drums").unwrap().unwrap();
        let scanned = |name: &str, v| Scanned {
            folder_id: folder,
            path: format!("/lib/Drums/{name}"),
            size: 1,
            mtime: 1,
            result: Ok(analyzed(v)),
        };
        let (stored, _) = db::store(
            &mut conn,
            vec![scanned("Kick.wav", 0.1), scanned("Snare.wav", 0.2)],
            0,
        )
        .unwrap();
        let (kick, snare) = (stored[0].id, stored[1].id);

        let engine = Engine::open(&db_path, None, |_| {}).unwrap();
        assert_eq!(engine.search(&query(false), 0, 10).total, 2);
        engine.set_favorite(kick, true).unwrap();
        assert_eq!(engine.view().version, 1);
        assert_eq!(engine.search(&query(true), 0, 10).total, 1);

        // A rebuild from a scan, which never heard of the favorite.
        let (again, _) = db::store(&mut conn, vec![scanned("Kick.wav", 0.3)], 0).unwrap();
        assert_eq!(again[0].id, kick);
        engine.publish(again, &[], None);
        assert_eq!(engine.view().version, 2);
        let favorites = engine.search(&query(true), 0, 10);
        assert_eq!(favorites.rows.len(), 1);
        assert!(favorites.rows[0].favorite);

        // The filtered search notices the next favorite too.
        engine.set_favorite(snare, true).unwrap();
        assert_eq!(engine.search(&query(true), 0, 10).total, 2);
        assert_eq!(
            engine.thumbnails(&[kick, 999]).unwrap()[..THUMB_BYTES],
            [1; THUMB_BYTES]
        );
        assert!(engine.waveform(999, 100).is_err());
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn chosen_categories_and_what_was_heard_outlast_rescans() {
        let dir = std::env::temp_dir().join(format!("samplary-chosen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("library.sqlite");
        let mut conn = db::open(&db_path).unwrap();
        let folder = db::add_folder(&mut conn, "/lib/Drums").unwrap().unwrap();
        let scanned = |mtime| Scanned {
            folder_id: folder,
            path: "/lib/Drums/Canned Crash.wav".into(),
            size: 1,
            mtime,
            result: Ok(analyzed(0.1)),
        };
        let (stored, _) = db::store(&mut conn, vec![scanned(1)], 0).unwrap();
        let id = stored[0].id;
        assert_eq!(db::unheard(&conn).unwrap().len(), 1);
        let heard = Embedding::new(&[0.5; crate::clap::DIMS]);
        db::store_embeddings(&mut conn, &[(id, heard.clone())]).unwrap();
        assert!(db::unheard(&conn).unwrap().is_empty());

        let engine = Engine::open(&db_path, None, |_| {}).unwrap();
        let category = || {
            let row = &engine.search(&query(false), 0, 10).rows[0];
            (row.category, row.chosen.is_some())
        };
        assert_eq!(category(), (Some("cymbal"), false));
        assert_eq!(
            engine.view().library.get(id).unwrap().embedding,
            Some(heard)
        );
        let chosen = Chosen {
            category: "fx".into(),
            subcategory: Some("impact".into()),
        };
        engine.set_category(id, Some(chosen.clone())).unwrap();
        assert_eq!(category(), (Some("fx"), true));

        // The file changed: it's listened to again, and stays as chosen.
        let (again, _) = db::store(&mut conn, vec![scanned(2)], 0).unwrap();
        assert_eq!(again[0].chosen, Some(chosen));
        assert_eq!(db::unheard(&conn).unwrap().len(), 1);
        engine.publish(again, &[], None);
        assert_eq!(category(), (Some("fx"), true));
        let reopened = Engine::open(&db_path, None, |_| {}).unwrap();
        assert_eq!(
            reopened.search(&query(false), 0, 10).rows[0].subcategory,
            Some("impact")
        );

        engine.set_category(id, None).unwrap();
        assert_eq!(category(), (Some("cymbal"), false));

        // Analyzed again with the file as it was: what was heard stays.
        let heard = Embedding::new(&[0.25; crate::clap::DIMS]);
        db::store_embeddings(&mut conn, &[(id, heard.clone())]).unwrap();
        let (again, _) = db::store(&mut conn, vec![scanned(2)], 0).unwrap();
        assert_eq!(again[0].embedding, Some(heard));
        assert!(db::unheard(&conn).unwrap().is_empty());
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_left_with_a_folder_stays_gone() {
        let dir = std::env::temp_dir().join(format!("samplary-refolder-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Drums")).unwrap();
        let drums = dir.join("Drums").canonicalize().unwrap();
        let db_path = dir.join("library.sqlite");
        let engine = Engine::open(&db_path, None, |_| {}).unwrap();
        let path = drums.to_string_lossy().into_owned();
        engine.add_folders(std::slice::from_ref(&path)).unwrap();
        let folder = engine.view().library.folders[0].id;

        // A scan stores what it heard, but hasn't published it yet...
        let mut conn = db::open(&db_path).unwrap();
        let scanned = Scanned {
            folder_id: folder,
            path: format!("{path}/Kick.wav"),
            size: 1,
            mtime: 1,
            result: Ok(analyzed(0.1)),
        };
        let (stored, _) = db::store(&mut conn, vec![scanned], 0).unwrap();
        assert_eq!(stored.len(), 1);

        // ...when its folder leaves, and comes back as a new one.
        engine.remove_folder(folder).unwrap();
        engine.add_folders(std::slice::from_ref(&path)).unwrap();
        assert_ne!(engine.view().library.folders[0].id, folder);
        engine.publish_stored(stored, &[]).unwrap();
        assert_eq!(engine.search(&query(false), 0, 10).total, 0);
        drop(conn);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
