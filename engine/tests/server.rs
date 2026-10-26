//! Runs the engine as the app does, over stdin and stdout, on a library of
//! a few small WAVs.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// A mono 16-bit WAV: a decaying tone, or noise when `hz` is 0.
fn write_wav(path: &Path, hz: f32, seconds: f32) {
    let sr = 44100u32;
    let n = (sr as f32 * seconds) as usize;
    let mut seed = 1u32;
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + n as u32 * 2).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(sr.to_le_bytes());
    b.extend((sr * 2).to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((n as u32 * 2).to_le_bytes());
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let v = if hz > 0.0 {
            (2.0 * std::f32::consts::PI * hz * t).sin()
        } else {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            seed as f32 / u32::MAX as f32 * 2.0 - 1.0
        };
        let v = v * (-t * 6.0).exp() * 0.8;
        b.extend(((v * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

/// Bytes that came back, and what was said about them.
type Bytes = Option<(Value, Vec<u8>)>;

enum Message {
    Reply(u64, Result<Value, String>, Bytes),
    Overview(Value),
    Progress(Value),
}

/// The engine, killed when the test ends however it ends.
struct Engine {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Message>,
    next_id: u64,
    overview: Value,
    progress: Value,
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Engine {
    fn start(db: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_samplary-engine"))
            .arg("serve")
            .arg(db)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut out = BufReader::new(stdout);
            let mut line = String::new();
            while out.read_line(&mut line).unwrap_or(0) > 0 {
                let m: Value = serde_json::from_str(&line).unwrap();
                line.clear();
                if m["event"] == "overview" {
                    let _ = tx.send(Message::Overview(m["data"].clone()));
                    continue;
                }
                if m["event"] == "progress" {
                    let _ = tx.send(Message::Progress(m["data"].clone()));
                    continue;
                }
                let id = m["id"].as_u64().unwrap();
                let message = if let Some(n) = m["bytes"].as_u64() {
                    let mut bytes = vec![0; n as usize];
                    out.read_exact(&mut bytes).unwrap();
                    Message::Reply(id, Ok(Value::Null), Some((m["meta"].clone(), bytes)))
                } else if let Some(e) = m["error"].as_str() {
                    Message::Reply(id, Err(e.to_string()), None)
                } else {
                    Message::Reply(id, Ok(m["ok"].clone()), None)
                };
                let _ = tx.send(message);
            }
        });
        Self {
            stdin: child.stdin.take(),
            child,
            messages,
            next_id: 1,
            overview: Value::Null,
            progress: Value::Null,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({ "id": id, "method": method, "params": params }).to_string();
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
        id
    }

    fn next(&mut self, deadline: Instant) -> Message {
        let left = deadline.saturating_duration_since(Instant::now());
        let m = self
            .messages
            .recv_timeout(left)
            .expect("the engine went quiet");
        match &m {
            Message::Overview(o) => self.overview = o.clone(),
            Message::Progress(p) => self.progress = p.clone(),
            Message::Reply(..) => {}
        }
        m
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.call_for(method, params).0
    }

    fn call_for(&mut self, method: &str, params: Value) -> (Result<Value, String>, Bytes) {
        let id = self.send(method, params);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Message::Reply(n, result, bytes) = self.next(deadline) {
                assert_eq!(n, id, "replies to one call at a time");
                return (result, bytes);
            }
        }
    }

    fn bytes(&mut self, method: &str, params: Value) -> (Value, Vec<u8>) {
        let (result, bytes) = self.call_for(method, params);
        result.unwrap();
        bytes.expect("bytes")
    }

    /// The first overview `ok` is happy with, the latest one included.
    fn overview_where(&mut self, ok: impl Fn(&Value) -> bool) -> Value {
        self.overview_while(ok, false)
    }

    /// The first overview `ok` is happy with once no scan is under way.
    fn scanned_where(&mut self, ok: impl Fn(&Value) -> bool) -> Value {
        self.overview_while(ok, true)
    }

    fn overview_while(&mut self, ok: impl Fn(&Value) -> bool, scanned: bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ok(&self.overview) || (scanned && !self.progress.is_null()) {
            if let Message::Reply(..) = self.next(deadline) {
                panic!("a reply nobody asked for");
            }
        }
        self.overview.clone()
    }
}

fn query(more: Value) -> Value {
    let mut q = json!({
        "text": "", "folder": null, "dir": null, "category": null, "subcategory": null, "kind": null,
        "favorites": false, "similar": null, "weights": [1, 1, 1, 1],
        "sort": "name", "descending": false, "seed": 0,
    });
    for (k, v) in more.as_object().unwrap() {
        q[k] = v.clone();
    }
    q
}

#[test]
fn serves_the_library() {
    let tmp = std::env::temp_dir().join(format!("samplary-serve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("Drums/Kicks")).unwrap();
    std::fs::create_dir_all(tmp.join("Elsewhere")).unwrap();
    let tmp: PathBuf = tmp.canonicalize().unwrap();
    let lib = tmp.join("Drums");
    write_wav(&lib.join("Kicks/Kick 01.wav"), 60.0, 0.5);
    write_wav(&lib.join("Kicks/Kick 02.wav"), 66.0, 0.5);
    write_wav(&lib.join("Hat 01.wav"), 0.0, 0.25);
    write_wav(&tmp.join("Elsewhere/Bass C2.wav"), 65.4, 0.5);
    std::fs::write(tmp.join("Elsewhere/Broken.wav"), b"not a sound").unwrap();

    let started = Instant::now();
    let mut engine = Engine::start(&tmp.join("library.sqlite"));
    let first = engine.overview_where(|o| !o.is_null());
    assert_eq!(first["total"], 0);
    eprintln!("first overview after {:?}", started.elapsed());

    // A folder joins: it shows at once, then fills in.
    assert_eq!(
        engine.call("addFolders", json!({ "paths": [lib] })),
        Ok(Value::Null)
    );
    let o = engine.scanned_where(|o| o["total"] == 3);
    assert_eq!(o["folders"][0]["name"], "Drums");
    assert_eq!(
        o["folders"][0]["dirs"],
        json!([{ "path": "Kicks", "count": 2 }])
    );
    let folder = o["folders"][0]["id"].clone();
    assert!(
        engine
            .call("addFolders", json!({ "paths": ["/no/such/folder"] }))
            .is_err()
    );

    // Pages of a search.
    let page = |engine: &mut Engine, offset: u64, limit: u64| {
        let r = engine
            .call(
                "search",
                json!({ "query": query(json!({})), "offset": offset, "limit": limit }),
            )
            .unwrap();
        let names: Vec<String> = r["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect();
        (r["total"].as_u64().unwrap(), names, r)
    };
    let (total, names, first_page) = page(&mut engine, 0, 2);
    assert_eq!(
        (total, names),
        (3, vec!["Hat 01.wav".into(), "Kick 01.wav".into()])
    );
    let (total, names, _) = page(&mut engine, 2, 2);
    assert_eq!((total, names), (3, vec!["Kick 02.wav".to_string()]));
    let kick = first_page["rows"][1].clone();
    assert_eq!(kick["folder"], "Drums/Kicks");
    assert_eq!(kick["category"], "kick");
    let kick_id = kick["id"].as_i64().unwrap();

    // Bytes: thumbnails, a waveform and the audio.
    let (_, thumbs) = engine.bytes("thumbnails", json!({ "ids": [kick_id, 999] }));
    assert_eq!(thumbs.len(), 2 * 1280);
    assert!(thumbs[..1280].iter().any(|&b| b != 0));
    assert!(thumbs[1280..].iter().all(|&b| b == 0));
    let (_, wave) = engine.bytes("waveform", json!({ "id": kick_id, "columns": 100 }));
    assert_eq!(wave.len(), 100 * 5);
    let (meta, pcm) = engine.bytes("pcm", json!({ "id": kick_id }));
    assert_eq!(meta, json!({ "sampleRate": 44100, "channels": 1 }));
    assert_eq!(pcm.len(), 22050 * 4);
    assert!(
        engine
            .call("waveform", json!({ "id": 999, "columns": 100 }))
            .is_err()
    );

    // Favorites count, without the results changing.
    let version = engine.overview["version"].clone();
    engine
        .call("setFavorite", json!({ "id": kick_id, "favorite": true }))
        .unwrap();
    let o = engine.overview_where(|o| o["favorites"] == 1);
    assert_eq!(o["version"], version);
    let favorites = engine
        .call(
            "search",
            json!({ "query": query(json!({ "favorites": true })), "offset": 0, "limit": 10 }),
        )
        .unwrap();
    assert_eq!(favorites["total"], 1);
    assert_eq!(favorites["rows"][0]["favorite"], true);

    // Dropped sounds: one from the library, a guest, and one that isn't.
    let dropped = engine
        .call(
            "dropPaths",
            json!({ "paths": [lib.join("Kicks/Kick 01.wav")] }),
        )
        .unwrap();
    assert_eq!(dropped["id"], kick_id);
    let guest = engine
        .call(
            "dropPaths",
            json!({ "paths": [tmp.join("Elsewhere/Bass C2.wav")] }),
        )
        .unwrap();
    assert_eq!(
        (guest["id"].clone(), guest["note"].clone()),
        (json!(-1), json!("C2"))
    );
    let similar = engine
        .call(
            "search",
            json!({ "query": query(json!({ "similar": -1 })), "offset": 0, "limit": 10 }),
        )
        .unwrap();
    assert_eq!(similar["total"], 3);
    assert!(similar["rows"][0]["similarity"].as_f64().unwrap() > 0.0);
    let (_, thumb) = engine.bytes("thumbnails", json!({ "ids": [-1] }));
    assert!(thumb.iter().any(|&b| b != 0));
    assert!(
        engine
            .call(
                "dropPaths",
                json!({ "paths": [tmp.join("Elsewhere/Broken.wav")] })
            )
            .is_err()
    );
    assert_eq!(
        engine.call("dropPaths", json!({ "paths": ["/no/such.wav"] })),
        Ok(Value::Null)
    );

    // A new file shows up without a rescan.
    write_wav(&lib.join("Kicks/Kick 03.wav"), 72.0, 0.5);
    let o = engine.scanned_where(|o| o["total"] == 4);
    assert_eq!(
        o["folders"][0]["dirs"],
        json!([{ "path": "Kicks", "count": 3 }])
    );
    // And a deleted one leaves.
    std::fs::remove_file(lib.join("Hat 01.wav")).unwrap();
    engine.overview_where(|o| o["total"] == 3);

    assert_eq!(engine.call("rescan", Value::Null), Ok(Value::Null));
    assert!(engine.call("nonsense", Value::Null).is_err());
    assert!(engine.call("search", json!({ "offset": 0 })).is_err());

    // The folder leaves, and its samples with it.
    assert_eq!(
        engine.call("removeFolder", json!({ "id": folder })),
        Ok(Value::Null)
    );
    let o = engine.overview_where(|o| o["total"] == 0);
    assert_eq!(o["folders"], json!([]));

    // Closing stdin ends it.
    drop(engine.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    while engine.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the engine outlived stdin");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::remove_dir_all(&tmp).unwrap();
}
