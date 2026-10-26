//! Serving the app over stdin and stdout, one JSON message per line.
//!
//! The app asks `{"id": n, "method": "...", "params": {...}}` and hears back
//! `{"id": n, "ok": ...}` or `{"id": n, "error": "why"}`. Bytes come back as
//! `{"id": n, "bytes": N, "meta": ...}`, a newline, then exactly N bytes.
//! In between, the engine says `{"event": "overview", "data": ...}`
//! whenever the library's overview changes, and `{"event": "progress",
//! "data": ...}` as a scan moves along. Logs go to stderr.

use std::io::{BufRead, BufWriter, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::db;
use crate::decode::Pcm;
use crate::engine::{Engine, spawn};
use crate::library::Query;
use crate::names;

#[derive(Deserialize)]
struct Request {
    id: u64,
    method: String,
    #[serde(default)]
    params: Value,
}

enum Reply {
    Json(Value),
    Bytes(Vec<u8>),
    /// Each channel in turn, as f32 LE.
    Pcm(Pcm),
}

/// Serves the library kept at `db_path`, listening with the CLAP model at
/// `model`, until stdin closes.
pub fn run(db_path: &Path, model: Option<&Path>) -> Result<(), String> {
    let engine = Engine::start(db_path, model, |event| send(json!(event), None))?;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("can't read request {line:?}: {e}");
                continue;
            }
        };
        let engine = engine.clone();
        // Each request gets a thread of its own, so a long decode never holds
        // up a search. Not a pool's: a pool thread waiting on a parallel sort
        // would run other requests meanwhile, and one needing the order that
        // sort is building would wait on itself forever.
        spawn(&request.method.clone(), move || {
            let reply = catch_unwind(AssertUnwindSafe(|| {
                handle(&engine, &request.method, request.params)
            }))
            .unwrap_or_else(|panic| {
                let why = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                Err(format!("something went wrong: {why}"))
            });
            reply_to(request.id, reply);
        });
    }
    Ok(())
}

fn params<T: DeserializeOwned>(params: Value) -> Result<T, String> {
    serde_json::from_value(params).map_err(|e| format!("bad params: {e}"))
}

fn handle(engine: &Arc<Engine>, method: &str, p: Value) -> Result<Reply, String> {
    #[derive(Deserialize)]
    struct Paths {
        paths: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Id {
        id: i64,
    }
    #[derive(Deserialize)]
    struct Search {
        query: Query,
        offset: usize,
        limit: usize,
    }
    #[derive(Deserialize)]
    struct Favorite {
        id: i64,
        favorite: bool,
    }
    #[derive(Deserialize)]
    struct Category {
        id: i64,
        category: Option<String>,
        subcategory: Option<String>,
    }
    #[derive(Deserialize)]
    struct Ids {
        ids: Vec<i64>,
    }
    #[derive(Deserialize)]
    struct Waveform {
        id: i64,
        columns: usize,
    }
    let json = |v: Result<Value, serde_json::Error>| v.map(Reply::Json).map_err(|e| e.to_string());
    match method {
        "addFolders" => {
            engine.add_folders(&params::<Paths>(p)?.paths)?;
            Ok(Reply::Json(Value::Null))
        }
        "removeFolder" => {
            engine.remove_folder(params::<Id>(p)?.id)?;
            Ok(Reply::Json(Value::Null))
        }
        "rescan" => {
            engine.rescan();
            Ok(Reply::Json(Value::Null))
        }
        "dropPaths" => json(serde_json::to_value(
            engine.drop_paths(&params::<Paths>(p)?.paths)?,
        )),
        "search" => {
            let s: Search = params(p)?;
            json(serde_json::to_value(
                engine.search(&s.query, s.offset, s.limit),
            ))
        }
        "setFavorite" => {
            let f: Favorite = params(p)?;
            engine.set_favorite(f.id, f.favorite)?;
            Ok(Reply::Json(Value::Null))
        }
        "setCategory" => {
            let c: Category = params(p)?;
            let chosen = db::chosen(c.category, c.subcategory);
            json(serde_json::to_value(engine.set_category(c.id, chosen)?))
        }
        "categories" => Ok(Reply::Json(
            names::category_names()
                .enumerate()
                .map(|(c, name)| {
                    let kinds: Vec<&str> = names::subcategory_names(c as u16).collect();
                    json!({ "name": name, "subcategories": kinds })
                })
                .collect(),
        )),
        "thumbnails" => Ok(Reply::Bytes(engine.thumbnails(&params::<Ids>(p)?.ids)?)),
        "waveform" => {
            let w: Waveform = params(p)?;
            Ok(Reply::Bytes(engine.waveform(w.id, w.columns)?))
        }
        "pcm" => Ok(Reply::Pcm(engine.pcm(params::<Id>(p)?.id)?)),
        _ => Err(format!("unknown method {method:?}")),
    }
}

fn reply_to(id: u64, reply: Result<Reply, String>) {
    match reply {
        Ok(Reply::Json(ok)) => send(json!({ "id": id, "ok": ok }), None),
        Ok(Reply::Bytes(bytes)) => send(
            json!({ "id": id, "bytes": bytes.len(), "meta": null }),
            Some(&|out| out.write_all(&bytes)),
        ),
        Ok(Reply::Pcm(pcm)) => {
            let frames = pcm.channels.first().map_or(0, Vec::len);
            let header = json!({
                "id": id,
                "bytes": frames * pcm.channels.len() * 4,
                "meta": { "sampleRate": pcm.sample_rate, "channels": pcm.channels.len() },
            });
            send(
                header,
                Some(&|out| {
                    for channel in &pcm.channels {
                        for v in channel {
                            out.write_all(&v.to_le_bytes())?;
                        }
                    }
                    Ok(())
                }),
            )
        }
        Err(error) => send(json!({ "id": id, "error": error }), None),
    }
}

type Body<'a> = &'a dyn Fn(&mut dyn Write) -> std::io::Result<()>;

/// Writes a message, and the bytes after it if it has any, whole: under
/// stdout's lock, then flushed.
fn send(message: Value, body: Option<Body>) {
    let stdout = std::io::stdout();
    let mut out = BufWriter::with_capacity(1 << 16, stdout.lock());
    let written = (|| {
        serde_json::to_writer(&mut out, &message)?;
        out.write_all(b"\n")?;
        if let Some(body) = body {
            body(&mut out)?;
        }
        out.flush()
    })();
    // The app has gone, and stdin closes next.
    if let Err(e) = written {
        eprintln!("can't write to the app: {e}");
    }
}
