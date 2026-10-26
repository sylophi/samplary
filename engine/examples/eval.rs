//! Scores the analysis against what a corpus's file names say.
//!
//!     cargo run --release --example eval -- <corpus dir> [query names...]
//!
//! Categories, notes, tempos, keys and loop/one-shot come from the names,
//! and the report says how often the audio agrees. Files are analyzed as a
//! scan does, and listened to with CLAP when its model is in `models/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;
use samplary_engine::analysis::Descriptor;
use samplary_engine::clap::{Clap, Embedding};
use samplary_engine::classify::{self, Sound, Space};
use samplary_engine::db::Analyzed;
use samplary_engine::index::Scales;
use samplary_engine::names::{self, Kind, NameInfo};
use samplary_engine::pitch::{note_name, pitch_class_name};
use samplary_engine::scan;

const MODEL: &str = "models/clap-audio.onnx";
const AUDIO: &[&str] = &["wav", "aif", "aiff", "flac", "mp3", "ogg"];

struct Item {
    /// Relative to the corpus.
    path: PathBuf,
    name: NameInfo,
    analysis: Analyzed,
    embedding: Option<Embedding>,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| AUDIO.contains(&e.to_ascii_lowercase().as_str()))
        {
            out.push(p);
        }
    }
}

fn pct(a: usize, b: usize) -> String {
    if b == 0 {
        "-".into()
    } else {
        format!("{:.1}% ({a}/{b})", 100.0 * a as f64 / b as f64)
    }
}

fn key_name(k: Option<(usize, bool)>) -> String {
    k.map_or("none".into(), |(pc, minor)| {
        format!("{}{}", pitch_class_name(pc), if minor { "m" } else { "" })
    })
}

fn off_octave(diff: f32) -> bool {
    (diff / 12.0 - (diff / 12.0).round()).abs() * 12.0 < 0.5
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(
        args.first()
            .expect("usage: eval <corpus dir> [query names...]"),
    );
    let queries = &args[1..];
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    paths.sort();

    let start = Instant::now();
    let results: Vec<Result<Item, (PathBuf, String)>> = paths
        .par_iter()
        .map(|p| {
            let analysis = scan::analyze_file(p).map_err(|e| (p.clone(), e))?;
            let rel = p.strip_prefix(&root).unwrap_or(p);
            let folders: Vec<&str> = rel
                .parent()
                .map(|f| f.iter().filter_map(|s| s.to_str()).collect())
                .unwrap_or_default();
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            Ok(Item {
                path: rel.to_path_buf(),
                name: names::parse(stem, &folders),
                analysis,
                embedding: None,
            })
        })
        .collect();
    let elapsed = start.elapsed();
    let (items, failures): (Vec<_>, Vec<_>) = results.into_iter().partition(|r| r.is_ok());
    let mut items: Vec<Item> = items.into_iter().map(|r| r.ok().unwrap()).collect();
    println!(
        "analyzed {} files in {:.1}s ({:.1} ms/file across threads), {} failed",
        items.len(),
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / items.len().max(1) as f64,
        failures.len()
    );
    for f in failures.iter().take(10) {
        let (p, e) = f.as_ref().err().unwrap();
        println!("  failed: {} ({e})", p.display());
    }

    if Path::new(MODEL).exists() {
        let clap = Clap::load(Path::new(MODEL)).unwrap();
        let start = Instant::now();
        items.par_iter_mut().for_each(|it| {
            let path = root.join(&it.path);
            it.embedding = scan::listen_file(&clap, &path).ok();
        });
        let elapsed = start.elapsed();
        println!(
            "listened to {} files in {:.1}s ({:.1} ms/file across threads)",
            items.iter().filter(|it| it.embedding.is_some()).count(),
            elapsed.as_secs_f64(),
            elapsed.as_secs_f64() * 1000.0 / items.len().max(1) as f64
        );
    } else {
        println!("no {MODEL}: guessing from the analysis alone");
    }

    let descriptors: Vec<&Descriptor> = items.iter().map(|it| &it.analysis.descriptor).collect();
    let scales = Scales::new(&descriptors);
    let embeddings: Vec<&Embedding> = items
        .iter()
        .filter_map(|it| it.embedding.as_ref())
        .collect();
    let space = Space {
        scales: &scales,
        heard: classify::typical_heard(&embeddings),
    };
    let heard: Vec<Sound> = items
        .iter()
        .map(|it| Sound {
            descriptor: &it.analysis.descriptor,
            embedding: it.embedding.as_ref(),
        })
        .collect();
    let analyzed: Vec<Sound> = heard
        .iter()
        .map(|s| Sound {
            embedding: None,
            ..*s
        })
        .collect();

    // Categories.
    let cats: Vec<&str> = names::category_names().collect();
    // Each sample in a group of its own, and each in its folder's.
    let alone: Vec<usize> = (0..items.len()).collect();
    let mut folder_ids = std::collections::HashMap::new();
    let by_folder: Vec<usize> = items
        .iter()
        .map(|it| {
            let n = folder_ids.len();
            *folder_ids.entry(it.path.parent()).or_insert(n)
        })
        .collect();
    let labels: Vec<Option<u16>> = items
        .iter()
        .map(|it| it.name.category.and_then(names::category_index))
        .collect();
    println!(
        "\ncategory from audio, leave-one-out over {} named samples:",
        labels.iter().flatten().count()
    );
    for (label, sounds, w) in [
        ("heard", &heard, classify::EQUAL),
        ("analyzed", &analyzed, classify::EQUAL),
        ("spectrum", &analyzed, [1.0, 0.0, 0.0, 0.0]),
        ("timbre", &analyzed, [0.0, 1.0, 0.0, 0.0]),
        ("pitch", &analyzed, [0.0, 0.0, 1.0, 0.0]),
        ("amplitude", &analyzed, [0.0, 0.0, 0.0, 1.0]),
    ] {
        let start = Instant::now();
        let r = classify::leave_one_out(
            space,
            sounds,
            &labels,
            &alone,
            &w,
            classify::CATEGORY_AGREEMENT,
        );
        let right = r.iter().filter(|(t, g)| Some(*t) == *g).count();
        let abstained = r.iter().filter(|(_, g)| g.is_none()).count();
        println!(
            "  {label:>10}: {} right, {} abstained ({:.1}s)",
            pct(right, r.len()),
            abstained,
            start.elapsed().as_secs_f64()
        );
        if label != "heard" {
            continue;
        }
        let mut per: BTreeMap<&str, (usize, usize, BTreeMap<&str, usize>)> = BTreeMap::new();
        for (t, g) in &r {
            let e = per.entry(cats[*t as usize]).or_default();
            e.1 += 1;
            if Some(*t) == *g {
                e.0 += 1;
            } else {
                *e.2.entry(g.map_or("?", |g| cats[g as usize])).or_default() += 1;
            }
        }
        for (c, (right, n, wrong)) in per {
            let mut wrong: Vec<_> = wrong.into_iter().collect();
            wrong.sort_by_key(|w| std::cmp::Reverse(w.1));
            let confusions: Vec<String> = wrong
                .iter()
                .take(3)
                .map(|(c, n)| format!("{c} {n}"))
                .collect();
            println!(
                "      {c:>11}: {:>18}   misses: {}",
                pct(right, n),
                confusions.join(", ")
            );
        }
    }

    // Subcategories, among the samples of each category.
    println!(
        "\nsubcategory from audio, among each category's named samples, leaving out the one or its folder:"
    );
    for (c, cat) in cats.iter().enumerate() {
        let c = c as u16;
        let types: Vec<&str> = names::subcategory_names(c).collect();
        if types.is_empty() {
            continue;
        }
        let members: Vec<usize> = (0..items.len())
            .filter(|&i| items[i].name.category == Some(cat))
            .collect();
        let labels: Vec<Option<u16>> = members
            .iter()
            .map(|&i| {
                let t = items[i].name.subcategory?;
                names::subcategory_index(c, t).map(u16::from)
            })
            .collect();
        let sounds: Vec<Sound> = members.iter().map(|&i| heard[i]).collect();
        let score = |groups: &[usize]| {
            let groups: Vec<usize> = members.iter().map(|&i| groups[i]).collect();
            let r = classify::leave_one_out(
                space,
                &sounds,
                &labels,
                &groups,
                &classify::EQUAL,
                classify::SUBCATEGORY_AGREEMENT,
            );
            let right = r.iter().filter(|(t, g)| Some(*t) == *g).count();
            let wrong = r.iter().filter(|(t, g)| g.is_some_and(|g| g != *t)).count();
            format!("right {}, wrong {wrong}", pct(right, r.len()))
        };
        let counts: Vec<String> = types
            .iter()
            .enumerate()
            .map(|(t, name)| {
                let n = labels.iter().filter(|l| **l == Some(t as u16)).count();
                format!("{name} {n}")
            })
            .collect();
        println!(
            "  {cat:>11}: {} named of {}  [{}]\n               one out: {}; folder out: {}",
            labels.iter().flatten().count(),
            members.len(),
            counts.join(", "),
            score(&alone),
            score(&by_folder)
        );
    }

    // Pitch.
    let named_notes: Vec<(&Item, f32)> = items
        .iter()
        .filter_map(|it| it.name.note.map(|n| (it, n)))
        .collect();
    let (mut exact, mut octave, mut unpitched) = (0, 0, 0);
    let mut misses = Vec::new();
    for (it, note) in &named_notes {
        match it.analysis.pitch {
            None => unpitched += 1,
            Some(p) if (p - note).abs() < 0.5 => exact += 1,
            Some(p) => {
                if off_octave(p - note) {
                    octave += 1;
                }
                misses.push(format!("{} heard {}", it.path.display(), note_name(p)));
            }
        }
    }
    println!(
        "\npitch vs note in name: exact {}, octave off {}, no pitch {}",
        pct(exact, named_notes.len()),
        pct(octave, named_notes.len()),
        pct(unpitched, named_notes.len())
    );
    let mut by_pack: BTreeMap<String, (usize, usize, usize, usize)> = BTreeMap::new();
    for (it, note) in &named_notes {
        let pack = it
            .path
            .iter()
            .take(2)
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let e = by_pack.entry(pack).or_default();
        e.0 += 1;
        match it.analysis.pitch {
            None => e.3 += 1,
            Some(p) if (p - note).abs() < 0.5 => e.1 += 1,
            Some(p) if off_octave(p - note) => e.2 += 1,
            _ => {}
        }
    }
    for (pack, (n, exact, octave, none)) in by_pack {
        println!("    {pack:<60} n {n:>4}  exact {exact:>4}  octave {octave:>4}  none {none:>4}");
    }
    if std::env::var("PITCH_MISSES").is_ok() {
        for m in &misses {
            println!("    {m}");
        }
    }

    // Tempo.
    let named_bpm: Vec<(&Item, f32)> = items
        .iter()
        .filter_map(|it| it.name.bpm.map(|b| (it, b)))
        .collect();
    let (mut right, mut related, mut none) = (0, 0, 0);
    let mut misses = Vec::new();
    for (it, bpm) in &named_bpm {
        match it.analysis.tempo {
            None => none += 1,
            Some(t) if (t - bpm).abs() <= 1.0 => right += 1,
            Some(t) if (t / bpm - 2.0).abs() < 0.02 || (t / bpm - 0.5).abs() < 0.01 => related += 1,
            Some(t) => misses.push(format!("{} heard {t:.1}", it.path.display())),
        }
    }
    println!(
        "\ntempo vs bpm in name: within 1 BPM {}, half/double {}, none {}",
        pct(right, named_bpm.len()),
        pct(related, named_bpm.len()),
        pct(none, named_bpm.len())
    );
    for m in misses.iter().take(8) {
        println!("    {m}");
    }

    // Key, leaving out drums as the app does.
    let named_keys: Vec<(&Item, (usize, bool))> = items
        .iter()
        .filter(|it| !it.name.category.is_some_and(names::is_drum))
        .filter_map(|it| it.name.key.map(|k| (it, k)))
        .collect();
    let right = named_keys
        .iter()
        .filter(|(it, k)| it.analysis.key == Some(*k))
        .count();
    let relative = named_keys
        .iter()
        .filter(|(it, (pc, minor))| {
            let rel = if *minor {
                ((pc + 3) % 12, false)
            } else {
                ((pc + 9) % 12, true)
            };
            it.analysis.key == Some(rel)
        })
        .count();
    let keyless = named_keys
        .iter()
        .filter(|(it, _)| it.analysis.key.is_none())
        .count();
    for (it, k) in named_keys.iter().take(25) {
        println!(
            "    {} named {} heard {}",
            it.path.display(),
            key_name(Some(*k)),
            key_name(it.analysis.key)
        );
    }
    println!(
        "\nkey vs key in name: exact {}, relative major/minor {}, none {}",
        pct(right, named_keys.len()),
        pct(relative, named_keys.len()),
        pct(keyless, named_keys.len())
    );

    // Loops.
    let kinds: Vec<(&Item, Kind)> = items
        .iter()
        .filter_map(|it| it.name.kind.map(|k| (it, k)))
        .collect();
    let loops = kinds.iter().filter(|(_, k)| *k == Kind::Loop).count();
    let loops_heard = kinds
        .iter()
        .filter(|(it, k)| *k == Kind::Loop && it.analysis.looped)
        .count();
    let shots_heard = kinds
        .iter()
        .filter(|(it, k)| *k == Kind::OneShot && !it.analysis.looped)
        .count();
    println!(
        "\nloop detection: named loops heard as loops {}, named one-shots heard as one-shots {}",
        pct(loops_heard, loops),
        pct(shots_heard, kinds.len() - loops)
    );
    for (it, _) in kinds
        .iter()
        .filter(|(it, k)| *k == Kind::Loop && !it.analysis.looped)
        .take(12)
    {
        let e = it.analysis.descriptor.env();
        let tempo = it
            .analysis
            .tempo
            .map_or("none".into(), |t| format!("{t:.1}"));
        println!(
            "    missed loop {}: tempo {tempo}, active {:.2}s, tail {:.0} dB, onsets/s {:.1}",
            it.path.display(),
            e[16].exp() - 0.02,
            e[20] * 60.0 - 60.0,
            e[19].exp() - 1.0
        );
    }

    // Queries.
    let mut picks: Vec<usize> = queries
        .iter()
        .filter_map(|q| {
            items
                .iter()
                .position(|it| it.path.to_string_lossy().contains(q.as_str()))
        })
        .collect();
    if picks.is_empty() {
        let mut seen = std::collections::HashSet::new();
        for (i, it) in items.iter().enumerate() {
            if let Some(c) = it.name.category
                && seen.insert(c)
            {
                picks.push(i);
            }
        }
    }
    for q in picks.into_iter().take(12) {
        let query = scales.query(descriptors[q]);
        let mut ranked: Vec<(usize, f32)> = (0..items.len())
            .filter(|&i| i != q)
            .map(|i| (i, query.distance_to(descriptors[i], &classify::EQUAL)))
            .collect();
        ranked.sort_by(|a, b| a.1.total_cmp(&b.1));
        println!("\nnearest to {}:", items[q].path.display());
        for (i, d) in ranked.iter().take(6) {
            let a = query.aspects(&scales.point(descriptors[*i]));
            println!(
                "    {d:.2}  [s {:.2} t {:.2} p {:.2} a {:.2}]  {}",
                a[0],
                a[1],
                a[2],
                a[3],
                items[*i].path.display()
            );
        }
    }
}
