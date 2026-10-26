//! Bringing the library up to date with what's in its folders: walking
//! them, analyzing what's new or changed, and forgetting what's gone. Then
//! having CLAP listen to what it hasn't heard.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rayon::ThreadPool;
use rayon::prelude::*;

use crate::analysis::{self, ANALYSIS_VERSION};
use crate::clap::{self, Clap, Embedding};
use crate::db::{self, Analyzed, Scanned};
use crate::engine::{Engine, Progress, Stage};
use crate::{decode, waveform};

const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "wave", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "caf",
];
/// Longer files are drawn and played from their first ten minutes.
pub const MAX_DECODE_SECONDS: f64 = 600.0;
/// How often new samples show up while a big folder is still being read.
const PUBLISH_EVERY: Duration = Duration::from_millis(1500);

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Runs `f`, turning a panic into an error: a decoder or analysis that
/// falls over on a strange file fails just that file.
fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err("this file couldn't be read".into()))
}

/// Decodes a file for drawing.
pub fn decode(path: &Path) -> Result<decode::Decoded, String> {
    guarded(|| decode::decode(path, Some(MAX_DECODE_SECONDS)))
}

/// Decodes a file's channels for playing.
pub fn decode_channels(path: &Path) -> Result<decode::Pcm, String> {
    guarded(|| decode::decode_channels(path, Some(MAX_DECODE_SECONDS)))
}

pub fn analyze_file(path: &Path) -> Result<Analyzed, String> {
    guarded(|| {
        let d = decode::decode(path, Some(MAX_DECODE_SECONDS))?;
        let head = (analysis::MAX_SECONDS * d.sample_rate as f64) as usize;
        let a = analysis::analyze(
            &d.mono[..d.mono.len().min(head)],
            d.sample_rate,
            d.duration(),
        );
        Ok(Analyzed {
            duration: d.duration(),
            sample_rate: d.sample_rate,
            channels: d.channels,
            bits: d.bits,
            pitch: a.pitch,
            key: a.key,
            tempo: a.tempo,
            looped: a.looped,
            descriptor: a.descriptor,
            waveform: waveform::render(&d.mono, d.sample_rate, waveform::THUMB_COLUMNS),
        })
    })
}

/// Where CLAP hears the start of a file.
pub fn listen_file(clap: &Clap, path: &Path) -> Result<Embedding, String> {
    guarded(|| {
        let d = decode::decode(path, Some(clap::SECONDS))?;
        clap.embed(&d.mono, d.sample_rate)
    })
}

/// What a scan reads: every library folder, or directories within them.
#[derive(Clone, Debug, PartialEq)]
pub enum Roots {
    All,
    /// Folder ids, and a directory within each.
    Dirs(Vec<(i64, PathBuf)>),
}

impl Roots {
    /// Both at once: everything covers any part, and a directory covers
    /// those within it.
    pub fn merge(self, other: Roots) -> Roots {
        match (self, other) {
            (Roots::Dirs(mut a), Roots::Dirs(b)) => {
                a.extend(b);
                Roots::Dirs(outermost(a))
            }
            _ => Roots::All,
        }
    }
}

/// `dirs` less those within another of the same folder. Another folder's
/// may be one that has left, with the same path as one that came after.
fn outermost(mut dirs: Vec<(i64, PathBuf)>) -> Vec<(i64, PathBuf)> {
    // A directory sorts before those within it.
    dirs.sort();
    dirs.dedup();
    let mut out: Vec<(i64, PathBuf)> = Vec::with_capacity(dirs.len());
    for (folder, dir) in dirs {
        if !out.iter().any(|(f, d)| *f == folder && dir.starts_with(d)) {
            out.push((folder, dir));
        }
    }
    out
}

/// The audio files under `root` with their sizes and modification times,
/// and the paths under it that couldn't be read.
fn files_in(root: &Path) -> (Vec<(PathBuf, i64, i64)>, Vec<PathBuf>) {
    let mut unreadable = Vec::new();
    let files = walkdir::WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0 || !(name.starts_with('.') || name == "__MACOSX")
        })
        .filter_map(|e| {
            e.map_err(|e| unreadable.push(e.path().unwrap_or(root).to_path_buf()))
                .ok()
        })
        .filter(|e| e.file_type().is_file() && is_audio(e.path()))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let mtime = meta
                .modified()
                .ok()?
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_secs() as i64;
            Some((e.into_path(), meta.len() as i64, mtime))
        })
        .collect();
    (files, unreadable)
}

/// `dir`, or the nearest directory above it that's still there, short of
/// leaving `root`: what's gone from a deleted directory is gone from the
/// one it was in.
fn still_there(dir: &Path, root: &Path) -> PathBuf {
    let mut dir = dir;
    while dir != root && !dir.exists() {
        match dir.parent() {
            Some(p) if p.starts_with(root) => dir = p,
            _ => return root.to_path_buf(),
        }
    }
    dir.to_path_buf()
}

struct Todo {
    folder_id: i64,
    path: String,
    size: i64,
    mtime: i64,
}

/// Brings `roots` up to date: forgets the files that are gone from them,
/// and analyzes those that are new or changed on `pool`, publishing as it
/// goes.
pub fn run(engine: &Engine, roots: Roots, pool: &ThreadPool) -> Result<(), String> {
    let mut conn = db::open(engine.db_path()).map_err(err)?;
    let folders = db::folders(&conn).map_err(err)?;
    let dirs: Vec<(i64, PathBuf)> = match roots {
        Roots::All => folders
            .iter()
            .map(|f| (f.id, PathBuf::from(&f.path)))
            .collect(),
        Roots::Dirs(dirs) => outermost(
            dirs.into_iter()
                .filter_map(|(id, dir)| {
                    let folder = folders.iter().find(|f| f.id == id)?;
                    Some((id, still_there(&dir, Path::new(&folder.path))))
                })
                .collect(),
        ),
    };

    let mut todo = Vec::new();
    let mut gone = Vec::new();
    for (folder_id, dir) in &dirs {
        let known = db::known_under(&conn, &dir.to_string_lossy()).map_err(err)?;
        let (files, unreadable) = files_in(dir);
        let mut seen = HashSet::new();
        // An unplugged drive or a folder without permission is no reason to
        // forget what was in it.
        seen.extend(
            known
                .keys()
                .filter(|p| unreadable.iter().any(|u| Path::new(p).starts_with(u)))
                .cloned(),
        );
        for (path, size, mtime) in files {
            let path = path.to_string_lossy().into_owned();
            let fresh = known.get(&path).is_some_and(|k| {
                k.size == size && k.mtime == mtime && k.version == ANALYSIS_VERSION
            });
            if !fresh {
                todo.push(Todo {
                    folder_id: *folder_id,
                    path: path.clone(),
                    size,
                    mtime,
                });
            }
            seen.insert(path);
        }
        gone.extend(
            known
                .iter()
                .filter(|(path, _)| !seen.contains(*path))
                .map(|(_, k)| k.id),
        );
    }
    if !gone.is_empty() {
        db::delete_samples(&mut conn, &gone).map_err(err)?;
        engine.publish(Vec::new(), &gone, None);
    }
    if todo.is_empty() {
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (mut upserts, mut failed) = (Vec::new(), Vec::new());
    in_batches(
        engine,
        pool,
        Stage::Analyzing,
        todo,
        |t| {
            // A folder that left the library mid-scan isn't read.
            Some(engine.has_folder(t.folder_id).then(|| Scanned {
                result: analyze_file(Path::new(&t.path)),
                folder_id: t.folder_id,
                path: t.path,
                size: t.size,
                mtime: t.mtime,
            }))
        },
        PUBLISH_EVERY,
        |batch, publish| {
            if !batch.is_empty() {
                let (s, f) = db::store(&mut conn, batch.into_iter().flatten().collect(), now)
                    .map_err(err)?;
                upserts.extend(s);
                failed.extend(f);
            }
            if publish && (!upserts.is_empty() || !failed.is_empty()) {
                engine.publish_stored(std::mem::take(&mut upserts), &failed)?;
                failed.clear();
            }
            Ok(())
        },
    )
}

/// Has CLAP, its model at `model`, listen on `pool` to the samples it
/// hasn't heard, publishing as it goes. Stops early for a scan waiting to
/// run, so new files show up first.
pub fn listen(engine: &Engine, model: &Path, pool: &ThreadPool) -> Result<(), String> {
    let mut conn = db::open(engine.db_path()).map_err(err)?;
    let todo = db::unheard(&conn).map_err(err)?;
    if todo.is_empty() || engine.scan_waiting() {
        return Ok(());
    }
    let clap = Clap::load(model)?;
    let mut heard = Vec::new();
    in_batches(
        engine,
        pool,
        Stage::Listening,
        todo,
        |(id, path)| {
            if engine.scan_waiting() {
                return None;
            }
            // One that can't be heard now, on a drive that's away, is tried
            // again next time.
            Some(listen_file(&clap, Path::new(&path)).ok().map(|e| (id, e)))
        },
        // Each publish starts the guesses over, and they take longer than a
        // scan's publishes do.
        4 * PUBLISH_EVERY,
        |batch, publish| {
            let batch: Vec<_> = batch.into_iter().flatten().collect();
            if !batch.is_empty() {
                db::store_embeddings(&mut conn, &batch).map_err(err)?;
                heard.extend(batch);
            }
            if publish && !heard.is_empty() {
                engine.publish_heard(std::mem::take(&mut heard));
            }
            Ok(())
        },
    )
}

/// Does `work` on each of `todo` on `pool`, telling how far `stage` has
/// got, and hands what it makes to `take` in batches at most 250 ms apart,
/// saying whether to publish: every `publish_every`, and with the last.
/// `work` returning `None` stops the rest, as does `take` failing.
fn in_batches<T: Send, R: Send>(
    engine: &Engine,
    pool: &ThreadPool,
    stage: Stage,
    todo: Vec<T>,
    work: impl Fn(T) -> Option<R> + Send + Sync,
    publish_every: Duration,
    mut take: impl FnMut(Vec<R>, bool) -> Result<(), String>,
) -> Result<(), String> {
    let total = todo.len();
    let progress = |done| engine.set_progress(Some(Progress { stage, done, total }));
    progress(0);
    std::thread::scope(|scope| {
        let (tx, rx) = mpsc::channel();
        scope.spawn(move || {
            // Stops early if taking below gives up.
            let _ = pool.install(|| {
                todo.into_par_iter()
                    .try_for_each_with(tx, |tx, t| tx.send(work(t).ok_or(())?).map_err(drop))
            });
        });

        let mut batch = Vec::new();
        let mut done = 0;
        let mut taken = Instant::now();
        let mut published = Instant::now();
        loop {
            let finished = match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(r) => {
                    batch.push(r);
                    false
                }
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => true,
            };
            if finished || batch.len() >= 64 || taken.elapsed() > Duration::from_millis(250) {
                done += batch.len();
                progress(done);
                let publish = finished || published.elapsed() > publish_every;
                take(std::mem::take(&mut batch), publish)?;
                taken = Instant::now();
                if publish {
                    published = Instant::now();
                }
            }
            if finished {
                return Ok(());
            }
        }
    })
}

fn err(e: impl ToString) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(d: &[(i64, &str)]) -> Roots {
        Roots::Dirs(d.iter().map(|(f, p)| (*f, PathBuf::from(p))).collect())
    }

    #[test]
    fn merges_roots() {
        assert_eq!(Roots::All.merge(dirs(&[(1, "/a")])), Roots::All);
        assert_eq!(dirs(&[(1, "/a/b")]).merge(Roots::All), Roots::All);
        assert_eq!(
            dirs(&[(1, "/a/b"), (2, "/c")]).merge(dirs(&[(1, "/a"), (1, "/a/bc")])),
            dirs(&[(1, "/a"), (2, "/c")])
        );
        assert_eq!(
            dirs(&[(1, "/a/b")]).merge(dirs(&[(1, "/a/b")])),
            dirs(&[(1, "/a/b")])
        );
        // A folder that left, and the same one added back.
        assert_eq!(
            dirs(&[(1, "/a")]).merge(dirs(&[(2, "/a")])),
            dirs(&[(1, "/a"), (2, "/a")])
        );
    }

    #[test]
    fn hears_audio_by_extension() {
        assert!(is_audio(Path::new("/x/Kick.WAV")));
        assert!(is_audio(Path::new("/x/loop.aif")));
        assert!(!is_audio(Path::new("/x/notes.txt")));
        assert!(!is_audio(Path::new("/x/wav")));
    }
}
