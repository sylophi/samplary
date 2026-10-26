//! Times what the app waits on, on a synthetic library.
//!
//!     cargo run --release --example bench -- [samples]
//!
//! Builds a library of 100,000 samples (or as many as asked) in a temporary
//! database, then times loading it, searching it, paging through a search,
//! the overview, a scan's publish and guessing categories.

use std::hint::black_box;
use std::time::{Duration, Instant};

use samplary_engine::analysis::{DESCRIPTOR_DIMS, Descriptor};
use samplary_engine::clap::{DIMS, Embedding};
use samplary_engine::db::{self, Analyzed, Scanned, Stored};
use samplary_engine::engine::Engine;
use samplary_engine::library::{Query, Sort};

const PACKS: usize = 10;
const NAMED: &[&str] = &[
    "Kick", "Snare", "Hat", "Clap", "Bass", "Pad", "Lead", "Pluck", "Vox", "FX", "Perc", "Tom",
];
const UNNAMED: &[&str] = &["Untitled", "Sound", "Recording", "Layer"];

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn float(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The `i`th sample: its path within its pack, and its analysis, near the
/// others of its kind.
fn sample(i: usize, random: &mut Random, centres: &[Vec<f32>]) -> (String, Analyzed) {
    let kind = random.below(NAMED.len() + 4);
    let word = NAMED.get(kind).unwrap_or(&UNNAMED[kind % UNNAMED.len()]);
    let looped = random.below(4) == 0;
    let name = if looped {
        format!("{word} Loop {} {}bpm", i % 97, 80 + random.below(100))
    } else {
        format!(
            "{word} {:02} {}",
            i % 101,
            ["A", "B", "Soft", "Hard"][random.below(4)]
        )
    };
    let dir = format!("Group {}/Set {}", random.below(20), random.below(10));
    let centre = &centres[kind % centres.len()];
    let descriptor = Descriptor(
        centre
            .iter()
            .map(|c| c + (random.float() - 0.5) * 0.3)
            .collect(),
    );
    let analyzed = Analyzed {
        duration: 0.1 + random.float() as f64 * 8.0,
        sample_rate: 44100,
        channels: 2,
        bits: Some(24),
        pitch: (random.below(2) == 0).then(|| 30.0 + random.float() * 60.0),
        key: (random.below(3) == 0).then(|| (random.below(12), random.below(2) == 0)),
        tempo: looped.then(|| 80.0 + random.float() * 100.0),
        looped,
        descriptor,
        waveform: (0..1280).map(|_| random.next() as u8).collect(),
    };
    (format!("{dir}/{name} {i}.wav"), analyzed)
}

/// What CLAP might make of a sample: near the others of its kind, as its
/// descriptor is.
fn heard(d: &Descriptor, random: &mut Random) -> Embedding {
    let point: Vec<f32> = (0..DIMS)
        .map(|k| d.0[k % d.0.len()] - 0.5 + (random.float() - 0.5) * 0.3)
        .collect();
    Embedding::new(&point)
}

/// The median of `runs` timings of `f`.
fn time<T>(runs: usize, mut f: impl FnMut() -> T) -> Duration {
    let mut times: Vec<Duration> = (0..runs)
        .map(|_| {
            let start = Instant::now();
            black_box(f());
            start.elapsed()
        })
        .collect();
    times.sort();
    times[runs / 2]
}

fn report(what: &str, d: Duration) {
    println!("{what:<48} {:>9.2} ms", d.as_secs_f64() * 1000.0);
}

fn query(f: impl FnOnce(&mut Query)) -> Query {
    let mut q = Query {
        text: String::new(),
        folder: None,
        dir: None,
        category: None,
        subcategory: None,
        kind: None,
        favorites: false,
        similar: None,
        weights: [1.0; 4],
        sort: Sort::Name,
        descending: false,
        seed: 0,
    };
    f(&mut q);
    q
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .map_or(100_000, |a| a.parse().expect("a number of samples"));
    let dir = std::env::temp_dir().join(format!("samplary-bench-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("library.sqlite");

    let start = Instant::now();
    let mut random = Random(0x2545_f491_4f6c_dd1d);
    let centres: Vec<Vec<f32>> = (0..NAMED.len())
        .map(|_| (0..DESCRIPTOR_DIMS).map(|_| random.float()).collect())
        .collect();
    let mut conn = db::open(&db_path).unwrap();
    let folders: Vec<(i64, String)> = (0..PACKS)
        .map(|p| {
            let path = format!("/bench/Pack {p}");
            (db::add_folder(&mut conn, &path).unwrap().unwrap(), path)
        })
        .collect();
    let mut i = 0;
    while i < n {
        let batch: Vec<Scanned> = (i..n.min(i + 5000))
            .map(|i| {
                let (folder_id, root) = &folders[i % PACKS];
                let (rel, analyzed) = sample(i, &mut random, &centres);
                Scanned {
                    folder_id: *folder_id,
                    path: format!("{root}/{rel}"),
                    size: 1,
                    mtime: 1,
                    result: Ok(analyzed),
                }
            })
            .collect();
        i += batch.len();
        let (stored, _) = db::store(&mut conn, batch, 0).unwrap();
        let heard: Vec<(i64, Embedding)> = stored
            .iter()
            .map(|s| (s.id, heard(&s.descriptor, &mut random)))
            .collect();
        db::store_embeddings(&mut conn, &heard).unwrap();
    }
    conn.execute("UPDATE samples SET favorite = 1 WHERE id % 50 = 0", [])
        .unwrap();
    println!(
        "made a library of {n} samples in {:.1}s\n",
        start.elapsed().as_secs_f64()
    );

    let start = Instant::now();
    let engine = Engine::open(&db_path, None, |overview| {
        black_box(serde_json::to_string(overview).unwrap());
    })
    .unwrap();
    report(
        "startup: load the library from the database",
        start.elapsed(),
    );
    let view = engine.view();
    let library = view.library.clone();
    println!(
        "  ({} samples, {} folders)",
        library.len(),
        library.folders.len()
    );

    let overview = || {
        let info = view.info(&Default::default());
        serde_json::to_string(&info).unwrap()
    };
    report("overview, first (counting folders)", time(1, overview));
    report("overview, after", time(9, overview));
    println!("  ({} KB of JSON)", overview().len() / 1024);

    let favorites = Default::default();
    let plain = query(|_| {});
    report(
        "sorted search, first (sorting by name)",
        time(1, || view.search(&plain, &favorites, None)),
    );
    report(
        "sorted search, by name",
        time(9, || view.search(&plain, &favorites, None)),
    );
    for (what, sort) in [
        ("duration", Sort::Duration),
        ("category", Sort::Category),
        ("folder", Sort::Folder),
    ] {
        let q = query(|q| q.sort = sort);
        report(
            &format!("sorted search, by {what}, first"),
            time(1, || view.search(&q, &favorites, None)),
        );
        report(
            &format!("sorted search, by {what}, descending"),
            time(9, || {
                view.search(
                    &query(|q| (q.sort, q.descending) = (sort, true)),
                    &favorites,
                    None,
                )
            }),
        );
    }
    let text = query(|q| q.text = "kick soft".into());
    report(
        "text search \"kick soft\"",
        time(9, || view.search(&text, &favorites, None)),
    );
    let text = query(|q| (q.text, q.folder) = ("loop 1".into(), Some(folders[3].0)));
    report(
        "text search \"loop 1\" in a folder",
        time(9, || view.search(&text, &favorites, None)),
    );
    let reference = library.samples()[n / 2].id;
    let similar = query(|q| q.similar = Some(reference));
    report(
        "similar search",
        time(9, || view.search(&similar, &favorites, None)),
    );
    let similar_snares = query(|q| (q.similar, q.text) = (Some(reference), "snare".into()));
    report(
        "similar search among \"snare\"",
        time(9, || view.search(&similar_snares, &favorites, None)),
    );

    let first = time(1, || engine.search(&similar, 0, 100));
    report("engine search, similar, first page", first);
    let mut offset = 0;
    report(
        "engine search, a later page, from the cache",
        time(9, || {
            offset += 100;
            engine.search(&similar, offset, 100)
        }),
    );

    // A scan's publish: a batch of new samples joins the library.
    let max_id = library.samples().last().map_or(0, |s| s.id);
    let mut upserts = Vec::new();
    for k in 0..500 {
        let (rel, a) = sample(n + k, &mut random, &centres);
        let (folder_id, root) = &folders[k % PACKS];
        upserts.push(Stored {
            id: max_id + 1 + k as i64,
            folder_id: *folder_id,
            path: format!("{root}/{rel}"),
            duration: a.duration,
            sample_rate: a.sample_rate,
            channels: a.channels,
            bits: a.bits,
            pitch: a.pitch,
            key: a.key,
            tempo: a.tempo,
            looped: a.looped,
            added_at: 1,
            descriptor: a.descriptor,
            embedding: None,
            chosen: None,
        });
    }
    let start = Instant::now();
    engine.publish(upserts, &[], None);
    report("scan-time publish of 500 new samples", start.elapsed());
    let view = engine.view();
    report(
        "  then a sorted search, by name, first",
        time(1, || view.search(&plain, &favorites, None)),
    );
    let start = Instant::now();
    engine.publish(Vec::new(), &[max_id + 1], None);
    report("scan-time publish of a removal", start.elapsed());
    let heard: Vec<(i64, Embedding)> = (max_id + 2..max_id + 501)
        .filter_map(|id| {
            let s = engine.view().library.get(id)?.descriptor.clone();
            Some((id, heard(&s, &mut random)))
        })
        .collect();
    let start = Instant::now();
    engine.publish_heard(heard);
    report("listening publish of 499 samples", start.elapsed());

    let library = engine.view().library.clone();
    let start = Instant::now();
    let guessed = library.guess(|| false).unwrap();
    report("background guess pass (all threads)", start.elapsed());
    println!(
        "  ({} unnamed, {} guessed)",
        library
            .samples()
            .iter()
            .filter(|s| s.category.is_none())
            .count(),
        guessed.iter().filter(|g| g.category.is_some()).count()
    );

    drop(engine);
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}
