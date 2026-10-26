//! The library on disk: its folders, and what each sample's analysis found.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Result, TransactionBehavior, params};
use serde::Serialize;

use crate::analysis::{ANALYSIS_VERSION, Descriptor};
use crate::clap::Embedding;

// AUTOINCREMENT, so ids are never reused: a scan's work for a folder that
// has left can't land in a new one, and the page caches samples by id.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS folders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS samples (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    version INTEGER NOT NULL,
    error TEXT,
    duration REAL,
    sample_rate INTEGER,
    channels INTEGER,
    bits INTEGER,
    pitch REAL,
    key_pc INTEGER,
    key_minor INTEGER,
    tempo REAL,
    looped INTEGER,
    descriptor BLOB,
    waveform BLOB,
    -- Where CLAP puts it, once it's listened; empty if it couldn't.
    embedding BLOB,
    favorite INTEGER NOT NULL DEFAULT 0,
    -- What the user says it is, over what its name says.
    chosen_category TEXT,
    chosen_subcategory TEXT,
    added_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS samples_folder ON samples(folder_id);
";

/// Columns libraries made before them don't have yet.
const ADDED: &[(&str, &str)] = &[
    ("embedding", "BLOB"),
    ("chosen_category", "TEXT"),
    ("chosen_subcategory", "TEXT"),
];

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
    )?;
    conn.execute_batch(SCHEMA)?;
    let columns: HashSet<String> = conn
        .prepare("SELECT name FROM pragma_table_info('samples')")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_>>()?;
    for (name, kind) in ADDED {
        if !columns.contains(*name) {
            conn.execute_batch(&format!("ALTER TABLE samples ADD COLUMN {name} {kind}"))?;
        }
    }
    // After the columns it needs: what CLAP has yet to hear, without reading
    // every row.
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS samples_unheard ON samples(id) WHERE embedding IS NULL",
    )?;
    Ok(conn)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    pub id: i64,
    pub path: String,
}

pub fn folders(conn: &Connection) -> Result<Vec<Folder>> {
    let mut stmt = conn.prepare("SELECT id, path FROM folders ORDER BY path")?;
    stmt.query_map([], |r| {
        Ok(Folder {
            id: r.get(0)?,
            path: r.get(1)?,
        })
    })?
    .collect()
}

/// Adds a library folder, returning its id. A folder inside one already in
/// the library adds nothing, and one that contains library folders absorbs
/// them and their samples.
pub fn add_folder(conn: &mut Connection, path: &str) -> Result<Option<i64>> {
    let existing = folders(conn)?;
    if existing
        .iter()
        .any(|f| path == f.path || Path::new(path).starts_with(&f.path))
    {
        return Ok(None);
    }
    let tx = conn.transaction()?;
    tx.execute("INSERT INTO folders (path) VALUES (?1)", params![path])?;
    let id = tx.last_insert_rowid();
    for child in existing
        .iter()
        .filter(|f| Path::new(&f.path).starts_with(path))
    {
        tx.execute(
            "UPDATE samples SET folder_id = ?1 WHERE folder_id = ?2",
            params![id, child.id],
        )?;
        tx.execute("DELETE FROM folders WHERE id = ?1", params![child.id])?;
    }
    tx.commit()?;
    Ok(Some(id))
}

pub fn remove_folder(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
    Ok(())
}

pub struct Known {
    pub id: i64,
    pub size: i64,
    pub mtime: i64,
    pub version: i64,
}

/// Every file a scan has seen under `dir`, by path.
pub fn known_under(conn: &Connection, dir: &str) -> Result<HashMap<String, Known>> {
    // Paths under "dir" are those from "dir/" up to "dir0", '0' being the
    // character after '/': a range the path index answers.
    let dir = dir.trim_end_matches('/');
    let mut stmt = conn.prepare(
        "SELECT path, id, size, mtime, version FROM samples WHERE path > ?1 AND path < ?2",
    )?;
    stmt.query_map(params![format!("{dir}/"), format!("{dir}0")], |r| {
        Ok((
            r.get(0)?,
            Known {
                id: r.get(1)?,
                size: r.get(2)?,
                mtime: r.get(3)?,
                version: r.get(4)?,
            },
        ))
    })?
    .collect()
}

/// Which of `ids` are still in the library.
pub fn existing(conn: &Connection, ids: &[i64]) -> Result<HashSet<i64>> {
    let mut stmt = conn.prepare_cached("SELECT 1 FROM samples WHERE id = ?1")?;
    let mut found = HashSet::new();
    for &id in ids {
        if stmt.exists(params![id])? {
            found.insert(id);
        }
    }
    Ok(found)
}

pub fn delete_samples(conn: &mut Connection, ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare("DELETE FROM samples WHERE id = ?1")?;
        for id in ids {
            stmt.execute(params![id])?;
        }
    }
    tx.commit()
}

pub struct Analyzed {
    pub duration: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: Option<u32>,
    pub pitch: Option<f32>,
    pub key: Option<(usize, bool)>,
    pub tempo: Option<f32>,
    pub looped: bool,
    pub descriptor: Descriptor,
    pub waveform: Vec<u8>,
}

pub struct Scanned {
    pub folder_id: i64,
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub result: std::result::Result<Analyzed, String>,
}

/// A category the user picked for a sample, and the kind of it, or none to
/// leave that to the name or the sound.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Chosen {
    pub category: String,
    pub subcategory: Option<String>,
}

/// A successfully analyzed sample, as stored.
#[derive(Clone)]
pub struct Stored {
    pub id: i64,
    pub folder_id: i64,
    pub path: String,
    pub duration: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: Option<u32>,
    pub pitch: Option<f32>,
    pub key: Option<(usize, bool)>,
    pub tempo: Option<f32>,
    pub looped: bool,
    pub added_at: i64,
    pub descriptor: Descriptor,
    pub embedding: Option<Embedding>,
    pub chosen: Option<Chosen>,
}

/// Inserts or refreshes analyses, keeping favorites, chosen categories, ids
/// and when each sample first arrived. A changed file is listened to again.
/// Samples of a folder removed mid-scan are dropped.
/// Returns the samples as stored, and the ids of the files that couldn't be
/// read.
pub fn store(
    conn: &mut Connection,
    batch: Vec<Scanned>,
    now: i64,
) -> Result<(Vec<Stored>, Vec<i64>)> {
    let mut stored = Vec::new();
    let mut failed = Vec::new();
    // Immediate: a transaction that reads first can't wait its turn to write.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    {
        let folders: HashSet<i64> = tx
            .prepare("SELECT id FROM folders")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_>>()?;
        let mut stmt = tx.prepare(
            "INSERT INTO samples (folder_id, path, size, mtime, version, error, duration,
                sample_rate, channels, bits, pitch, key_pc, key_minor, tempo, looped,
                descriptor, waveform, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
             ON CONFLICT(path) DO UPDATE SET
                folder_id = excluded.folder_id, size = excluded.size, mtime = excluded.mtime,
                version = excluded.version, error = excluded.error, duration = excluded.duration,
                sample_rate = excluded.sample_rate, channels = excluded.channels,
                bits = excluded.bits, pitch = excluded.pitch, key_pc = excluded.key_pc,
                key_minor = excluded.key_minor, tempo = excluded.tempo, looped = excluded.looped,
                descriptor = excluded.descriptor, waveform = excluded.waveform,
                embedding = CASE WHEN samples.size = excluded.size
                    AND samples.mtime = excluded.mtime THEN samples.embedding END
             RETURNING id, added_at, chosen_category, chosen_subcategory, embedding",
        )?;
        let ids = |r: &rusqlite::Row| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                chosen(r.get(2)?, r.get(3)?),
                r.get::<_, Option<Vec<u8>>>(4)?
                    .and_then(|b| Embedding::from_bytes(&b)),
            ))
        };
        for s in batch.into_iter().filter(|s| folders.contains(&s.folder_id)) {
            match s.result {
                Ok(a) => {
                    let (id, added_at, chosen, embedding) = stmt.query_row(
                        params![
                            s.folder_id,
                            s.path,
                            s.size,
                            s.mtime,
                            ANALYSIS_VERSION,
                            None::<String>,
                            a.duration,
                            a.sample_rate,
                            a.channels,
                            a.bits,
                            a.pitch,
                            a.key.map(|k| k.0 as i64),
                            a.key.map(|k| k.1),
                            a.tempo,
                            a.looped,
                            a.descriptor.to_bytes(),
                            a.waveform,
                            now,
                        ],
                        ids,
                    )?;
                    stored.push(Stored {
                        id,
                        folder_id: s.folder_id,
                        path: s.path,
                        duration: a.duration,
                        sample_rate: a.sample_rate,
                        channels: a.channels,
                        bits: a.bits,
                        pitch: a.pitch,
                        key: a.key,
                        tempo: a.tempo,
                        looped: a.looped,
                        added_at,
                        descriptor: a.descriptor,
                        embedding,
                        chosen,
                    });
                }
                Err(e) => {
                    let (id, ..) = stmt.query_row(
                        params![
                            s.folder_id,
                            s.path,
                            s.size,
                            s.mtime,
                            ANALYSIS_VERSION,
                            e,
                            None::<f64>,
                            None::<u32>,
                            None::<u16>,
                            None::<u32>,
                            None::<f32>,
                            None::<i64>,
                            None::<bool>,
                            None::<f32>,
                            None::<bool>,
                            None::<Vec<u8>>,
                            None::<Vec<u8>>,
                            now,
                        ],
                        ids,
                    )?;
                    failed.push(id);
                }
            }
        }
    }
    tx.commit()?;
    Ok((stored, failed))
}

/// Every sample analyzed by the current version of the analysis, by id.
pub fn samples(conn: &Connection) -> Result<Vec<Stored>> {
    let mut stmt = conn.prepare(
        "SELECT id, folder_id, path, duration, sample_rate, channels, bits, pitch, key_pc,
            key_minor, tempo, looped, added_at, descriptor, embedding, chosen_category,
            chosen_subcategory
         FROM samples WHERE error IS NULL AND version = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![ANALYSIS_VERSION], |r| {
        let key_pc: Option<i64> = r.get(8)?;
        let key_minor: Option<bool> = r.get(9)?;
        let descriptor: Vec<u8> = r.get(13)?;
        let embedding: Option<Vec<u8>> = r.get(14)?;
        let chosen = chosen(r.get(15)?, r.get(16)?);
        Ok(
            Descriptor::from_bytes(&descriptor).map(|descriptor| Stored {
                id: r.get(0).unwrap_or_default(),
                folder_id: r.get(1).unwrap_or_default(),
                path: r.get(2).unwrap_or_default(),
                duration: r.get(3).unwrap_or_default(),
                sample_rate: r.get(4).unwrap_or_default(),
                channels: r.get(5).unwrap_or_default(),
                bits: r.get(6).unwrap_or_default(),
                pitch: r.get(7).unwrap_or_default(),
                key: key_pc.zip(key_minor).map(|(pc, m)| (pc as usize, m)),
                tempo: r.get(10).unwrap_or_default(),
                looped: r.get(11).unwrap_or_default(),
                added_at: r.get(12).unwrap_or_default(),
                descriptor,
                embedding: embedding.and_then(|b| Embedding::from_bytes(&b)),
                chosen,
            }),
        )
    })?;
    Ok(rows.filter_map(|r| r.ok().flatten()).collect())
}

pub fn chosen(category: Option<String>, subcategory: Option<String>) -> Option<Chosen> {
    category.map(|category| Chosen {
        category,
        subcategory,
    })
}

/// The samples CLAP hasn't listened to yet, by id and path.
pub fn unheard(conn: &Connection) -> Result<Vec<(i64, String)>> {
    conn.prepare(
        "SELECT id, path FROM samples
         WHERE error IS NULL AND version = ?1 AND embedding IS NULL ORDER BY id",
    )?
    .query_map(params![ANALYSIS_VERSION], |r| Ok((r.get(0)?, r.get(1)?)))?
    .collect()
}

/// Keeps what CLAP made of samples, none for those it couldn't hear.
pub fn store_embeddings(conn: &mut Connection, heard: &[(i64, Embedding)]) -> Result<()> {
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare("UPDATE samples SET embedding = ?1 WHERE id = ?2")?;
        for (id, e) in heard {
            stmt.execute(params![e.to_bytes(), id])?;
        }
    }
    tx.commit()
}

/// Whether there was a sample `id` to set.
pub fn set_chosen(conn: &Connection, id: i64, chosen: Option<&Chosen>) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE samples SET chosen_category = ?1, chosen_subcategory = ?2 WHERE id = ?3",
        params![
            chosen.map(|c| &c.category),
            chosen.and_then(|c| c.subcategory.as_ref()),
            id
        ],
    )?;
    Ok(changed > 0)
}

pub fn favorites(conn: &Connection) -> Result<HashSet<i64>> {
    conn.prepare("SELECT id FROM samples WHERE favorite = 1")?
        .query_map([], |r| r.get(0))?
        .collect()
}

/// Whether there was a sample `id` to set.
pub fn set_favorite(conn: &Connection, id: i64, favorite: bool) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE samples SET favorite = ?1 WHERE id = ?2",
        params![favorite, id],
    )?;
    Ok(changed > 0)
}

pub fn thumbnail(conn: &Connection, id: i64) -> Result<Option<Vec<u8>>> {
    conn.prepare_cached("SELECT waveform FROM samples WHERE id = ?1")?
        .query_row(params![id], |r| r.get(0))
        .optional()
        .map(Option::flatten)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_libraries_get_the_new_columns() {
        let path =
            std::env::temp_dir().join(format!("samplary-older-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let older = SCHEMA
            .lines()
            .filter(|l| {
                !ADDED
                    .iter()
                    .any(|(name, _)| l.trim_start().starts_with(name))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&older).unwrap();
        conn.execute("INSERT INTO folders (path) VALUES ('/lib')", [])
            .unwrap();
        drop(conn);
        let conn = open(&path).unwrap();
        assert!(unheard(&conn).unwrap().is_empty());
        assert_eq!(folders(&conn).unwrap().len(), 1);
        drop(conn);
        std::fs::remove_file(&path).unwrap();
    }
}
