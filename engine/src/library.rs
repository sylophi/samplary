//! The library in memory: the analyzed samples, what their names and
//! folders say about them, and searching through them. A `Library` never
//! changes once made. A change makes a new one from the last, sharing the
//! samples it didn't touch.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::analysis::Descriptor;
use crate::clap::Embedding;
use crate::classify::{self, Sound, Space};
use crate::db::{Analyzed, Chosen, Folder, Stored};
use crate::index::{Scales, Weights};
use crate::names::{self, Kind};
use crate::pitch::{note_name, pitch_class_name};

/// What the analysis heard, before the name had its say: kept to read the
/// sample again when its library folder changes.
#[derive(Clone)]
struct Heard {
    pitch: Option<f32>,
    key: Option<(usize, bool)>,
    tempo: Option<f32>,
    looped: bool,
}

#[derive(Clone)]
pub struct Sample {
    pub id: i64,
    pub folder_id: i64,
    pub path: String,
    pub name: String,
    /// Folder within its library folder, "/"-separated, "" at the top.
    pub dir: String,
    pub duration: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: Option<u32>,
    pub pitch: Option<f32>,
    pub key: Option<(usize, bool)>,
    pub tempo: Option<f32>,
    pub kind: Kind,
    /// What the user, or else its name or folders, call it, as a place in
    /// `names::category_names`.
    pub category: Option<u16>,
    /// The kind of `category` they call it, as a place in
    /// `names::subcategory_names`.
    pub subcategory: Option<u8>,
    /// What the user called it, which `category` and `subcategory` follow.
    pub chosen: Option<Chosen>,
    pub added_at: i64,
    pub descriptor: Descriptor,
    pub embedding: Option<Embedding>,
    heard: Heard,
    haystack: String,
    name_key: Vec<u8>,
}

impl Sample {
    /// Reads what it can from a sample's name and the folders above it, up
    /// to its library folder.
    pub fn new(s: Stored, folder: &Folder) -> Self {
        let path = Path::new(&s.path);
        let rel = path.strip_prefix(&folder.path).unwrap_or(path);
        let dir_parts: Vec<&str> = rel
            .parent()
            .map(|p| p.iter().filter_map(|c| c.to_str()).collect())
            .unwrap_or_default();
        let root_name = Path::new(&folder.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let mut folder_words = vec![root_name];
        folder_words.extend(&dir_parts);
        let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
        // What the user called it, over what the name says.
        let chosen = s
            .chosen
            .as_ref()
            .and_then(|c| names::category_index(&c.category));
        let named = names::parse_as(stem, &folder_words, chosen.map(names::category_name));
        let kind = named
            .kind
            .unwrap_or(if s.looped { Kind::Loop } else { Kind::OneShot });
        // "drums" names a loop or a kit, so a one-shot from a drum folder is
        // better off with the category its sound suggests.
        let category = named
            .category
            .filter(|&c| chosen.is_some() || !(c == "drums" && kind == Kind::OneShot));
        let drums = named.category.is_some_and(names::is_drum);
        // Drum names are full of letters that look like keys or notes ("Db"
        // for dub, "Fm" for FM, "A1" for a variation): drums go without a
        // key, and their pitch is the one they sound at.
        let key = if drums {
            None
        } else {
            named.key.or(if kind == Kind::Loop { s.key } else { None })
        };
        let pitch = if drums { None } else { named.note }.or(s.pitch);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let dir = dir_parts.join("/");
        let category = category.and_then(names::category_index);
        let chosen_kind = s.chosen.as_ref().and_then(|c| c.subcategory.as_deref());
        let subcategory = category
            .zip(chosen_kind.or(named.subcategory))
            .and_then(|(c, t)| names::subcategory_index(c, t));
        Self {
            haystack: format!("{root_name}/{dir}/{name}").to_lowercase(),
            name_key: natural_key(&name),
            id: s.id,
            folder_id: folder.id,
            name,
            dir,
            duration: s.duration,
            sample_rate: s.sample_rate,
            channels: s.channels,
            bits: s.bits,
            pitch: if kind == Kind::OneShot { pitch } else { None },
            key,
            tempo: if kind == Kind::Loop {
                named.bpm.or(s.tempo)
            } else {
                None
            },
            kind,
            category,
            subcategory,
            chosen: s.chosen.filter(|_| chosen.is_some()),
            added_at: s.added_at,
            heard: Heard {
                pitch: s.pitch,
                key: s.key,
                tempo: s.tempo,
                looped: s.looped,
            },
            descriptor: s.descriptor,
            embedding: s.embedding,
            path: s.path,
        }
    }

    /// The sample as stored, in `folder_id`.
    pub fn stored(&self, folder_id: i64) -> Stored {
        Stored {
            id: self.id,
            folder_id,
            path: self.path.clone(),
            duration: self.duration,
            sample_rate: self.sample_rate,
            channels: self.channels,
            bits: self.bits,
            pitch: self.heard.pitch,
            key: self.heard.key,
            tempo: self.heard.tempo,
            looped: self.heard.looped,
            added_at: self.added_at,
            descriptor: self.descriptor.clone(),
            embedding: self.embedding.clone(),
            chosen: self.chosen.clone(),
        }
    }

    /// The sample read again as part of `folder`.
    fn moved(&self, folder: &Folder) -> Self {
        Self::new(self.stored(folder.id), folder)
    }
}

pub struct Library {
    pub folders: Vec<Folder>,
    /// Kept in id order.
    samples: Vec<Arc<Sample>>,
    ids: Vec<i64>,
    scales: Scales,
    /// Positions in name order, worked out when first needed. The other
    /// orders start from it.
    by_name: OnceLock<Vec<u32>>,
    folder_infos: OnceLock<Vec<FolderInfo>>,
}

impl Library {
    /// `stored` is in id order.
    pub fn new(folders: Vec<Folder>, stored: Vec<Stored>) -> Self {
        let samples = stored
            .into_par_iter()
            .filter_map(|s| {
                let folder = folders.iter().find(|f| f.id == s.folder_id)?;
                Some(Arc::new(Sample::new(s, folder)))
            })
            .collect();
        Self::from_samples(folders, samples)
    }

    fn from_samples(folders: Vec<Folder>, samples: Vec<Arc<Sample>>) -> Self {
        let descriptors: Vec<&Descriptor> = samples.iter().map(|s| &s.descriptor).collect();
        let scales = Scales::new(&descriptors);
        Self {
            ids: samples.iter().map(|s| s.id).collect(),
            scales,
            folders,
            samples,
            by_name: OnceLock::new(),
            folder_infos: OnceLock::new(),
        }
    }

    /// The library with its folders now `folders`: `upserts` join it or
    /// replace the samples with their ids, keeping what was chosen for them
    /// (it may have been since they were stored), and `removed` leave it.
    /// Samples belong to the folder their path is in, and leave with it.
    pub fn with_changes(
        &self,
        folders: Vec<Folder>,
        upserts: Vec<Stored>,
        removed: &HashSet<i64>,
    ) -> Self {
        let refolder = folders != self.folders;
        let mut upserts: HashMap<i64, Stored> = upserts.into_iter().map(|s| (s.id, s)).collect();
        let fresh = |s: Stored| {
            let folder = folder_of(&folders, &s.path)?;
            Some(Arc::new(Sample::new(s, folder)))
        };
        let mut kept = Vec::with_capacity(self.samples.len() + upserts.len());
        for s in &self.samples {
            if removed.contains(&s.id) {
                continue;
            }
            if let Some(mut stored) = upserts.remove(&s.id) {
                stored.chosen = s.chosen.clone();
                kept.extend(fresh(stored));
            } else if !refolder {
                kept.push(s.clone());
            } else {
                match folder_of(&folders, &s.path) {
                    Some(f) if f.id == s.folder_id => kept.push(s.clone()),
                    Some(f) => kept.push(Arc::new(s.moved(f))),
                    None => {}
                }
            }
        }
        let mut joined: Vec<Arc<Sample>> = upserts
            .into_values()
            .filter(|s| !removed.contains(&s.id))
            .collect::<Vec<_>>()
            .into_par_iter()
            .filter_map(fresh)
            .collect();
        joined.sort_by_key(|s| s.id);
        Self::from_samples(folders, merge_by_id(kept, joined))
    }

    /// The library with what CLAP heard in some of its samples.
    pub fn with_embeddings(&self, heard: Vec<(i64, Embedding)>) -> Self {
        let mut samples = self.samples.clone();
        for (id, embedding) in heard {
            if let Some(i) = self.position(id) {
                let mut s = (*samples[i]).clone();
                s.embedding = Some(embedding);
                samples[i] = Arc::new(s);
            }
        }
        Self::from_samples(self.folders.clone(), samples)
    }

    /// The library with `chosen` as what the sample with `id` is.
    pub fn with_chosen(&self, id: i64, chosen: Option<Chosen>) -> Self {
        let mut samples = self.samples.clone();
        if let Some(i) = self.position(id) {
            let s = &samples[i];
            let mut stored = s.stored(s.folder_id);
            stored.chosen = chosen;
            samples[i] = Arc::new(Sample::new(stored, self.folder(s.folder_id).unwrap()));
        }
        Self::from_samples(self.folders.clone(), samples)
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn samples(&self) -> &[Arc<Sample>] {
        &self.samples
    }

    pub fn position(&self, id: i64) -> Option<usize> {
        self.ids.binary_search(&id).ok()
    }

    pub fn get(&self, id: i64) -> Option<&Sample> {
        self.position(id).map(|i| &*self.samples[i])
    }

    pub fn find_path(&self, path: &str) -> Option<usize> {
        self.samples.iter().position(|s| s.path == path)
    }

    pub fn folder(&self, id: i64) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    /// The library folder `path` is in.
    pub fn folder_of(&self, path: &str) -> Option<&Folder> {
        folder_of(&self.folders, path)
    }

    /// Guesses what the samples' names don't say from their nearest named
    /// neighbours: categories, then subcategories among the samples of
    /// each. None if `cancelled` says to stop first.
    pub fn guess(&self, cancelled: impl Fn() -> bool + Sync) -> Option<Vec<Guess>> {
        let sounds: Vec<Sound> = self.samples.iter().map(|s| Sound::of(s)).collect();
        let heard: Vec<&Embedding> = sounds.iter().filter_map(|s| s.embedding).collect();
        let space = Space {
            scales: &self.scales,
            heard: classify::typical_heard(&heard),
        };
        let named: Vec<Option<u16>> = self.samples.iter().map(|s| s.category).collect();
        let categories = classify::propagate(
            space,
            &sounds,
            &named,
            classify::CATEGORY_AGREEMENT,
            &cancelled,
        )?;
        let mut guesses: Vec<Guess> = categories
            .iter()
            .map(|&category| Guess {
                category,
                subcategory: None,
            })
            .collect();
        let mut members: HashMap<u16, Vec<usize>> = HashMap::new();
        for (i, c) in named.iter().zip(&categories).enumerate() {
            if let Some(c) = c.0.or(*c.1) {
                members.entry(c).or_default().push(i);
            }
        }
        for (c, members) in members {
            if names::subcategory_names(c).next().is_none() {
                continue;
            }
            let sounds: Vec<Sound> = members.iter().map(|&i| sounds[i]).collect();
            let labels: Vec<Option<u16>> = members
                .iter()
                .map(|&i| self.samples[i].subcategory.map(u16::from))
                .collect();
            let subcategories = classify::propagate(
                space,
                &sounds,
                &labels,
                classify::SUBCATEGORY_AGREEMENT,
                &cancelled,
            )?;
            for (i, t) in members.into_iter().zip(subcategories) {
                guesses[i].subcategory = t.map(|t| t as u8);
            }
        }
        Some(guesses)
    }

    fn by_name(&self) -> &[u32] {
        self.by_name.get_or_init(|| {
            let mut order: Vec<u32> = (0..self.samples.len() as u32).collect();
            order.par_sort_unstable_by(|&a, &b| {
                let (x, y) = (&self.samples[a as usize], &self.samples[b as usize]);
                x.name_key
                    .cmp(&y.name_key)
                    .then_with(|| x.dir.cmp(&y.dir))
                    .then(a.cmp(&b))
            });
            order
        })
    }

    /// Positions ordered by `key`, then by name.
    fn ordered_by(&self, key: impl Fn(usize) -> u64 + Sync) -> Vec<u32> {
        let by_name = self.by_name();
        let mut keyed: Vec<(u64, u32)> = by_name
            .par_iter()
            .enumerate()
            .map(|(rank, &i)| (key(i as usize), rank as u32))
            .collect();
        keyed.par_sort_unstable();
        keyed
            .into_iter()
            .map(|(_, rank)| by_name[rank as usize])
            .collect()
    }

    fn folder_infos(&self) -> &[FolderInfo] {
        self.folder_infos.get_or_init(|| {
            let mut per_dir: HashMap<(i64, &str), usize> = HashMap::new();
            for s in &self.samples {
                *per_dir.entry((s.folder_id, &s.dir)).or_default() += 1;
            }
            // Each folder's own count, and its share of the ones above it.
            let mut counts: HashMap<i64, BTreeMap<&str, usize>> = HashMap::new();
            for ((folder, dir), n) in per_dir {
                let dirs = counts.entry(folder).or_default();
                *dirs.entry("").or_default() += n;
                for (i, _) in dir.match_indices('/') {
                    *dirs.entry(&dir[..i]).or_default() += n;
                }
                if !dir.is_empty() {
                    *dirs.entry(dir).or_default() += n;
                }
            }
            self.folders
                .iter()
                .map(|f| {
                    let dirs = counts.remove(&f.id).unwrap_or_default();
                    FolderInfo {
                        id: f.id,
                        name: Path::new(&f.path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(&f.path)
                            .to_string(),
                        path: f.path.clone(),
                        count: dirs.get("").copied().unwrap_or(0),
                        dirs: dirs
                            .into_iter()
                            .filter(|(d, _)| !d.is_empty())
                            .map(|(path, count)| DirInfo {
                                path: path.to_string(),
                                count,
                            })
                            .collect(),
                    }
                })
                .collect()
        })
    }
}

fn within(path: &str, dir: &str) -> bool {
    path.len() > dir.len()
        && path.starts_with(dir)
        && (dir.ends_with('/') || path.as_bytes()[dir.len()] == b'/')
}

fn folder_of<'a>(folders: &'a [Folder], path: &str) -> Option<&'a Folder> {
    folders.iter().find(|f| within(path, &f.path))
}

/// Two runs of samples in id order as one.
fn merge_by_id(a: Vec<Arc<Sample>>, b: Vec<Arc<Sample>>) -> Vec<Arc<Sample>> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut a, mut b) = (a.into_iter().peekable(), b.into_iter().peekable());
    while let (Some(x), Some(y)) = (a.peek(), b.peek()) {
        out.extend(if x.id < y.id { a.next() } else { b.next() });
    }
    out.extend(a);
    out.extend(b);
    out
}

/// What a sample's sound suggests its name doesn't say.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Guess {
    pub category: Option<u16>,
    pub subcategory: Option<u8>,
}

/// A library and what's guessed about it: what searches read.
pub struct View {
    pub library: Arc<Library>,
    /// Guessed from the sound, by position.
    guessed: Vec<Guess>,
    /// Counts up whenever search results could differ.
    pub version: u64,
    /// Positions in each sort's order but by name, worked out when first
    /// searched.
    orders: [OnceLock<Vec<u32>>; 7],
    categories: OnceLock<Vec<CategoryInfo>>,
}

impl View {
    pub fn new(library: Arc<Library>, guessed: Vec<Guess>, version: u64) -> Self {
        Self {
            library,
            guessed,
            version,
            orders: Default::default(),
            categories: OnceLock::new(),
        }
    }

    /// The next view, of `library`, keeping the guesses for the samples
    /// still in it.
    pub fn carry(&self, library: Arc<Library>) -> Self {
        let old = &self.library;
        let mut j = 0;
        let guessed = library
            .samples
            .iter()
            .map(|s| {
                while j < old.ids.len() && old.ids[j] < s.id {
                    j += 1;
                }
                // Guesses hold while the names say what they said.
                let same = j < old.ids.len()
                    && old.ids[j] == s.id
                    && (old.samples[j].category, old.samples[j].subcategory)
                        == (s.category, s.subcategory);
                if same {
                    self.guessed[j]
                } else {
                    Guess::default()
                }
            })
            .collect();
        Self::new(library, guessed, self.version + 1)
    }

    /// The next view, with these guesses, or None if they're what it has.
    pub fn with_guesses(&self, guessed: Vec<Guess>) -> Option<Self> {
        (guessed != self.guessed)
            .then(|| Self::new(self.library.clone(), guessed, self.version + 1))
    }

    fn category(&self, i: usize) -> Option<u16> {
        self.library.samples[i]
            .category
            .or(self.guessed[i].category)
    }

    fn subcategory(&self, i: usize) -> Option<u8> {
        self.library.samples[i]
            .subcategory
            .or(self.guessed[i].subcategory)
    }

    /// Every match for `q` in order. `guest` is the sound to compare with
    /// when `q.similar` is a guest's id.
    pub fn search(
        &self,
        q: &Query,
        favorites: &HashSet<i64>,
        guest: Option<&Descriptor>,
    ) -> Ranked {
        let lib = &*self.library;
        let reference = match q.similar {
            None => None,
            Some(id) => match guest.or_else(|| lib.get(id).map(|s| &s.descriptor)) {
                Some(d) => Some(d),
                // The sound to compare with has left the library.
                None => return Ranked::default(),
            },
        };
        let terms: Vec<String> = q
            .text
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        let category = q.category.as_deref().map(names::category_index);
        let subcategory = category
            .flatten()
            .zip(q.subcategory.as_deref())
            .map(|(c, t)| names::subcategory_index(c, t));
        let matches = |i: usize| {
            let s = &lib.samples[i];
            q.folder.is_none_or(|f| s.folder_id == f)
                && q.dir.as_deref().is_none_or(|d| {
                    s.dir == d || (s.dir.starts_with(d) && s.dir[d.len()..].starts_with('/'))
                })
                && category.is_none_or(|c| c.is_some() && self.category(i) == c)
                && subcategory.is_none_or(|t| t.is_some() && self.subcategory(i) == t)
                && q.kind.is_none_or(|k| s.kind == k)
                && (!q.favorites || favorites.contains(&s.id))
                && terms.iter().all(|t| s.haystack.contains(t.as_str()))
                && q.similar != Some(s.id)
        };
        let n = lib.samples.len();
        match reference {
            Some(reference) => {
                let query = lib.scales.query(reference);
                let mut ranked: Vec<(f32, u32)> = (0..n)
                    .into_par_iter()
                    .filter(|&i| matches(i))
                    .map(|i| {
                        let d = query.distance_to(&lib.samples[i].descriptor, &q.weights);
                        (d, i as u32)
                    })
                    .collect();
                ranked.par_sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                Ranked {
                    positions: ranked.iter().map(|r| r.1).collect(),
                    distances: Some(ranked.iter().map(|r| r.0).collect()),
                }
            }
            // Keyed by id, so samples keep their places as others come and go.
            None if q.sort == Sort::Shuffle => {
                let mut keyed: Vec<(u64, u32)> = (0..n)
                    .into_par_iter()
                    .filter(|&i| matches(i))
                    .map(|i| (mix(q.seed ^ lib.samples[i].id as u64), i as u32))
                    .collect();
                keyed.par_sort_unstable();
                Ranked {
                    positions: keyed.into_iter().map(|k| k.1).collect(),
                    distances: None,
                }
            }
            None => {
                let matched: Vec<bool> = (0..n).into_par_iter().map(matches).collect();
                let order = self.order(q.sort);
                let picked = |i: &&u32| matched[**i as usize];
                Ranked {
                    positions: if q.descending {
                        order.iter().rev().filter(picked).copied().collect()
                    } else {
                        order.iter().filter(picked).copied().collect()
                    },
                    distances: None,
                }
            }
        }
    }

    fn order(&self, sort: Sort) -> &[u32] {
        let lib = &*self.library;
        if sort == Sort::Name {
            return lib.by_name();
        }
        self.orders[sort as usize].get_or_init(|| {
            let mut dir_ranks: HashMap<&str, u64> = HashMap::new();
            if sort == Sort::Folder {
                // Folders whose names differ only as `natural` ignores
                // share a rank, so names decide between them.
                let dirs: HashSet<&str> = lib.samples.iter().map(|s| s.dir.as_str()).collect();
                let mut keyed: Vec<(Vec<u8>, &str)> =
                    dirs.into_iter().map(|d| (natural_key(d), d)).collect();
                keyed.sort();
                let mut rank = 0;
                for (k, (key, dir)) in keyed.iter().enumerate() {
                    if k > 0 && keyed[k - 1].0 != *key {
                        rank += 1;
                    }
                    dir_ranks.insert(dir, rank);
                }
            }
            let mut alphabetical: Vec<&str> = names::category_names().collect();
            alphabetical.sort();
            let category_rank: Vec<u64> = names::category_names()
                .map(|c| alphabetical.iter().position(|a| *a == c).unwrap() as u64)
                .collect();
            lib.ordered_by(|i| {
                let s = &lib.samples[i];
                match sort {
                    Sort::Name => 0,
                    Sort::Folder => dir_ranks[s.dir.as_str()],
                    Sort::Duration => ordered_f64(s.duration),
                    Sort::Pitch => missing_last(s.pitch),
                    Sort::Tempo => missing_last(s.tempo),
                    Sort::Category => self.category(i).map_or(u64::MAX, |c| {
                        category_rank[c as usize] << 8
                            | self.subcategory(i).unwrap_or(u8::MAX) as u64
                    }),
                    // Newest first.
                    Sort::Added => !((s.added_at as u64) ^ (1 << 63)),
                    Sort::Shuffle => unreachable!("shuffles aren't kept"),
                }
            })
        })
    }

    /// Rows `offset..offset + limit` of `ranked`.
    pub fn rows(
        &self,
        ranked: &Ranked,
        offset: usize,
        limit: usize,
        favorites: &HashSet<i64>,
    ) -> Vec<Row> {
        let end = offset.saturating_add(limit).min(ranked.positions.len());
        (offset.min(end)..end)
            .map(|k| {
                let distance = ranked.distances.as_ref().map(|d| d[k]);
                self.row(ranked.positions[k] as usize, distance, favorites)
            })
            .collect()
    }

    pub fn row(&self, i: usize, distance: Option<f32>, favorites: &HashSet<i64>) -> Row {
        let s = &self.library.samples[i];
        let folder = self
            .library
            .folder(s.folder_id)
            .and_then(|f| Path::new(&f.path).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("");
        row(
            s,
            folder,
            self.guessed[i],
            favorites.contains(&s.id),
            distance,
        )
    }

    pub fn info(&self, favorites: &HashSet<i64>) -> Info<'_> {
        let lib = &*self.library;
        Info {
            total: lib.len(),
            favorites: favorites
                .iter()
                .filter(|&&id| lib.position(id).is_some())
                .count(),
            folders: lib.folder_infos(),
            categories: self.categories.get_or_init(|| {
                let mut counts: Vec<(usize, Vec<usize>)> = (0..names::category_names().count())
                    .map(|c| (0, vec![0; names::subcategory_names(c as u16).count()]))
                    .collect();
                for i in 0..lib.len() {
                    if let Some(c) = self.category(i) {
                        let (count, subcategories) = &mut counts[c as usize];
                        *count += 1;
                        if let Some(t) = self.subcategory(i) {
                            subcategories[t as usize] += 1;
                        }
                    }
                }
                names::category_names()
                    .zip(counts)
                    .enumerate()
                    .filter(|(_, (_, (count, _)))| *count > 0)
                    .map(|(c, (name, (count, subcategories)))| CategoryInfo {
                        name,
                        count,
                        subcategories: names::subcategory_names(c as u16)
                            .zip(subcategories)
                            .filter(|(_, count)| *count > 0)
                            .map(|(name, count)| SubcategoryInfo { name, count })
                            .collect(),
                    })
                    .collect()
            }),
        }
    }
}

/// A search's matches in order, to page through.
#[derive(Default)]
pub struct Ranked {
    pub positions: Vec<u32>,
    /// How far each is from the sound searched for, when there is one.
    pub distances: Option<Vec<f32>>,
}

/// A row for a sound from outside the library, read as if its folder were
/// one of the library's.
pub fn guest_row(id: i64, path: &str, a: &Analyzed) -> Row {
    let folder = Folder {
        id: 0,
        path: Path::new(path)
            .parent()
            .map_or(String::new(), |p| p.to_string_lossy().into_owned()),
    };
    let stored = Stored {
        id,
        folder_id: 0,
        path: path.to_string(),
        duration: a.duration,
        sample_rate: a.sample_rate,
        channels: a.channels,
        bits: a.bits,
        pitch: a.pitch,
        key: a.key,
        tempo: a.tempo,
        looped: a.looped,
        added_at: 0,
        descriptor: a.descriptor.clone(),
        embedding: None,
        chosen: None,
    };
    let s = Sample::new(stored, &folder);
    let name = Path::new(&folder.path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    row(&s, name, Guess::default(), false, None)
}

fn row(s: &Sample, folder: &str, guess: Guess, favorite: bool, distance: Option<f32>) -> Row {
    let category = s.category.or(guess.category);
    let subcategory = s.subcategory.or(guess.subcategory);
    Row {
        id: s.id,
        name: s.name.clone(),
        path: s.path.clone(),
        folder: if s.dir.is_empty() {
            folder.to_string()
        } else {
            format!("{folder}/{}", s.dir)
        },
        duration: s.duration,
        sample_rate: s.sample_rate,
        channels: s.channels,
        bits: s.bits,
        category: category.map(names::category_name),
        category_guessed: s.category.is_none() && guess.category.is_some(),
        chosen: s.chosen.clone(),
        subcategory: category
            .zip(subcategory)
            .map(|(c, t)| names::subcategory_name(c, t)),
        subcategory_guessed: s.subcategory.is_none() && guess.subcategory.is_some(),
        looped: s.kind == Kind::Loop,
        note: s.pitch.map(note_name),
        key: s
            .key
            .map(|(pc, minor)| format!("{}{}", pitch_class_name(pc), if minor { "m" } else { "" })),
        tempo: s.tempo,
        favorite,
        similarity: distance.map(similarity),
    }
}

/// 1 for the same sound, about a third for two samples as far apart as
/// is typical in the library.
fn similarity(distance: f32) -> f32 {
    1.0 / (1.0 + (distance / 0.7).powi(2))
}

/// A key that sorts as names should: case-insensitively, with runs of
/// digits compared as numbers ("Kick 2" before "Kick 10"). A run becomes
/// '0', which stands where any digit would against other characters, then
/// its length without leading zeros, then its digits.
fn natural_key(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 8);
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c.is_ascii_digit() {
            let mut end = i + 1;
            while let Some(&(j, d)) = chars.peek().filter(|(_, d)| d.is_ascii_digit()) {
                end = j + d.len_utf8();
                chars.next();
            }
            let run = s[i..end].trim_start_matches('0');
            out.push(b'0');
            out.extend_from_slice(&(run.len() as u32).to_be_bytes());
            out.extend_from_slice(run.as_bytes());
        } else {
            let mut buf = [0; 4];
            for l in c.to_lowercase() {
                out.extend_from_slice(l.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out
}

/// SplitMix64's finalizer: scatters `x` across all 64 bits.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

/// Sorts as `f64::total_cmp` does.
fn ordered_f64(x: f64) -> u64 {
    let b = x.to_bits();
    if b >> 63 == 1 { !b } else { b | 1 << 63 }
}

/// Sorts as `f32::total_cmp` does, with none last.
fn missing_last(x: Option<f32>) -> u64 {
    x.map_or(u64::MAX, |x| {
        let b = x.to_bits();
        (if b >> 31 == 1 { !b } else { b | 1 << 31 }) as u64
    })
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Sort {
    Name,
    Folder,
    Duration,
    Pitch,
    Tempo,
    Category,
    Added,
    /// In an order of `Query::seed`'s.
    Shuffle,
}

#[derive(Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    pub text: String,
    pub folder: Option<i64>,
    pub dir: Option<String>,
    pub category: Option<String>,
    /// Only with `category`, one of its subcategories.
    pub subcategory: Option<String>,
    pub kind: Option<Kind>,
    pub favorites: bool,
    pub similar: Option<i64>,
    pub weights: Weights,
    pub sort: Sort,
    pub descending: bool,
    pub seed: u64,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub folder: String,
    pub duration: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits: Option<u32>,
    pub category: Option<&'static str>,
    pub category_guessed: bool,
    /// What the user chose it to be, over its name and sound.
    pub chosen: Option<Chosen>,
    pub subcategory: Option<&'static str>,
    pub subcategory_guessed: bool,
    pub looped: bool,
    pub note: Option<String>,
    pub key: Option<String>,
    pub tempo: Option<f32>,
    pub favorite: bool,
    pub similarity: Option<f32>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct DirInfo {
    pub path: String,
    pub count: usize,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct FolderInfo {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub count: usize,
    pub dirs: Vec<DirInfo>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct CategoryInfo {
    pub name: &'static str,
    pub count: usize,
    pub subcategories: Vec<SubcategoryInfo>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct SubcategoryInfo {
    pub name: &'static str,
    pub count: usize,
}

/// What's in the library: its counts, folders and categories.
#[derive(Serialize)]
pub struct Info<'a> {
    pub total: usize,
    pub favorites: usize,
    pub folders: &'a [FolderInfo],
    pub categories: &'a [CategoryInfo],
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DESCRIPTOR_DIMS;

    struct Random(u32);

    impl Random {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
            self.0 as f32 / u32::MAX as f32
        }

        fn descriptor(&mut self) -> Descriptor {
            Descriptor((0..DESCRIPTOR_DIMS).map(|_| self.next()).collect())
        }

        /// `d`, nudged a little.
        fn near(&mut self, d: &Descriptor) -> Descriptor {
            Descriptor(d.0.iter().map(|v| v + (self.next() - 0.5) * 0.01).collect())
        }
    }

    fn stored(id: i64, folder_id: i64, path: &str, descriptor: Descriptor) -> Stored {
        Stored {
            id,
            folder_id,
            path: path.into(),
            duration: 1.0,
            sample_rate: 44100,
            channels: 2,
            bits: Some(24),
            pitch: None,
            key: None,
            tempo: None,
            looped: false,
            added_at: id,
            descriptor,
            embedding: None,
            chosen: None,
        }
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

    fn folders() -> Vec<Folder> {
        vec![
            Folder {
                id: 1,
                path: "/lib/Drums".into(),
            },
            Folder {
                id: 2,
                path: "/lib/Synths".into(),
            },
        ]
    }

    struct Fixture {
        view: View,
        favorites: HashSet<i64>,
        random: Random,
    }

    const KICK_HARD: i64 = 1;
    const KICK_SOFT: i64 = 2;
    const PAD: i64 = 5;

    fn fixture() -> Fixture {
        let mut random = Random(7);
        let reference = random.descriptor();
        let kick_soft = random.near(&reference);
        let mut groove = stored(
            4,
            1,
            "/lib/Drums/Loops/Groove 120bpm.wav",
            random.descriptor(),
        );
        groove.looped = true;
        groove.tempo = Some(118.0);
        let mut snare = stored(3, 1, "/lib/Drums/Snares/Snare 3.wav", random.descriptor());
        snare.duration = 0.5;
        let samples = vec![
            stored(KICK_HARD, 1, "/lib/Drums/Kicks/Kick_Hard_01.wav", reference),
            stored(KICK_SOFT, 1, "/lib/Drums/Kicks/Kick_Soft_02.wav", kick_soft),
            snare,
            groove,
            stored(PAD, 2, "/lib/Synths/Pad F#min.wav", random.descriptor()),
        ];
        let library = Library::new(folders(), samples);
        Fixture {
            view: View::new(Arc::new(library), vec![Guess::default(); 5], 1),
            favorites: HashSet::from([KICK_HARD, PAD]),
            random,
        }
    }

    impl Fixture {
        fn names(&self, f: impl FnOnce(&mut Query)) -> Vec<String> {
            let ranked = self.view.search(&query(f), &self.favorites, None);
            self.view
                .rows(&ranked, 0, usize::MAX, &self.favorites)
                .into_iter()
                .map(|r| r.name)
                .collect()
        }

        fn row(&self, path: &str) -> Option<Row> {
            let i = self.view.library.find_path(path)?;
            Some(self.view.row(i, None, &self.favorites))
        }
    }

    #[test]
    fn filters() {
        let f = fixture();
        assert_eq!(f.names(|q| q.folder = Some(2)), ["Pad F#min.wav"]);
        assert_eq!(
            f.names(|q| q.dir = Some("Kicks".into())),
            ["Kick_Hard_01.wav", "Kick_Soft_02.wav"]
        );
        assert!(f.names(|q| q.dir = Some("Kick".into())).is_empty());
        assert_eq!(
            f.names(|q| q.category = Some("snare".into())),
            ["Snare 3.wav"]
        );
        assert!(f.names(|q| q.category = Some("nonsense".into())).is_empty());
        assert_eq!(
            f.names(|q| q.kind = Some(Kind::Loop)),
            ["Groove 120bpm.wav"]
        );
        assert_eq!(
            f.names(|q| q.favorites = true),
            ["Kick_Hard_01.wav", "Pad F#min.wav"]
        );
    }

    #[test]
    fn finds_every_term_in_the_folders_and_name() {
        let f = fixture();
        assert_eq!(
            f.names(|q| q.text = "KICK hard".into()),
            ["Kick_Hard_01.wav"]
        );
        assert_eq!(f.names(|q| q.text = "drums snare".into()), ["Snare 3.wav"]);
        assert!(f.names(|q| q.text = "synths kick".into()).is_empty());
    }

    #[test]
    fn sorts() {
        let f = fixture();
        assert_eq!(
            f.names(|q| q.descending = true),
            [
                "Snare 3.wav",
                "Pad F#min.wav",
                "Kick_Soft_02.wav",
                "Kick_Hard_01.wav",
                "Groove 120bpm.wav",
            ]
        );
        assert_eq!(f.names(|q| q.sort = Sort::Tempo)[0], "Groove 120bpm.wav");
        assert_eq!(f.names(|q| q.sort = Sort::Duration)[0], "Snare 3.wav");
        assert_eq!(f.names(|q| q.sort = Sort::Added)[0], "Pad F#min.wav");
        assert_eq!(
            f.names(|q| q.sort = Sort::Folder),
            [
                "Pad F#min.wav",
                "Kick_Hard_01.wav",
                "Kick_Soft_02.wav",
                "Groove 120bpm.wav",
                "Snare 3.wav",
            ]
        );
        assert_eq!(
            f.names(|q| q.sort = Sort::Category),
            [
                "Groove 120bpm.wav",
                "Kick_Hard_01.wav",
                "Kick_Soft_02.wav",
                "Pad F#min.wav",
                "Snare 3.wav",
            ]
        );
    }

    #[test]
    fn shuffles_by_seed() {
        let f = fixture();
        let shuffle = |seed| f.names(|q| (q.sort, q.seed) = (Sort::Shuffle, seed));
        let mut all = shuffle(1);
        assert_eq!(all, shuffle(1));
        assert!((2..10).any(|seed| shuffle(seed) != all));
        // A narrower search keeps the same order.
        let kicks = f.names(|q| (q.sort, q.seed, q.text) = (Sort::Shuffle, 1, "kick".into()));
        all.retain(|n| n.starts_with("Kick"));
        assert_eq!(kicks, all);
    }

    #[test]
    fn ranks_by_similarity_leaving_the_reference_out() {
        let f = fixture();
        let q = query(|q| q.similar = Some(KICK_HARD));
        let ranked = f.view.search(&q, &f.favorites, None);
        let rows = f.view.rows(&ranked, 0, 10, &f.favorites);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].id, KICK_SOFT);
        assert!(rows[0].similarity.unwrap() > 0.9);
        assert!(rows.iter().all(|r| r.id != KICK_HARD));
        assert!(rows[1].similarity < rows[0].similarity);
    }

    #[test]
    fn ranks_against_a_guest() {
        let mut f = fixture();
        let pad = &f.view.library.get(PAD).unwrap().descriptor;
        let guest = f.random.near(pad);
        let q = query(|q| q.similar = Some(-1));
        let ranked = f.view.search(&q, &f.favorites, Some(&guest));
        assert_eq!(ranked.positions.len(), 5);
        assert_eq!(
            f.view
                .row(ranked.positions[0] as usize, None, &f.favorites)
                .id,
            PAD
        );
    }

    #[test]
    fn finds_nothing_once_the_reference_is_gone() {
        let f = fixture();
        let ranked = f
            .view
            .search(&query(|q| q.similar = Some(999)), &f.favorites, None);
        assert!(ranked.positions.is_empty());
        let empty = View::new(
            Arc::new(Library::new(Vec::new(), Vec::new())),
            Vec::new(),
            1,
        );
        let ranked = empty.search(&query(|q| q.similar = Some(1)), &f.favorites, None);
        assert!(ranked.positions.is_empty());
    }

    #[test]
    fn pages() {
        let f = fixture();
        let ranked = f.view.search(&query(|_| {}), &f.favorites, None);
        assert_eq!(ranked.positions.len(), 5);
        let names = |offset, limit| -> Vec<String> {
            let rows = f.view.rows(&ranked, offset, limit, &f.favorites);
            rows.into_iter().map(|r| r.name).collect()
        };
        assert_eq!(names(1, 2), ["Kick_Hard_01.wav", "Kick_Soft_02.wav"]);
        assert_eq!(names(4, 10), ["Snare 3.wav"]);
        assert!(names(9, 10).is_empty());
    }

    #[test]
    fn reads_what_names_say() {
        let f = fixture();
        let groove = f.row("/lib/Drums/Loops/Groove 120bpm.wav").unwrap();
        assert_eq!(groove.folder, "Drums/Loops");
        assert_eq!(groove.category, Some("drums"));
        assert!(groove.looped);
        assert_eq!(groove.tempo, Some(120.0));
        let pad = f.row("/lib/Synths/Pad F#min.wav").unwrap();
        assert_eq!(
            (pad.category, pad.key.as_deref(), pad.note, pad.favorite),
            (Some("pad"), Some("F#m"), None, true)
        );
        assert!(f.row("/lib/Elsewhere.wav").is_none());
    }

    #[test]
    fn counts_folders_and_categories() {
        let f = fixture();
        let info = f.view.info(&f.favorites);
        assert_eq!((info.total, info.favorites), (5, 2));
        assert_eq!(
            info.folders[0],
            FolderInfo {
                id: 1,
                name: "Drums".into(),
                path: "/lib/Drums".into(),
                count: 4,
                dirs: ["Kicks", "Loops", "Snares"]
                    .iter()
                    .zip([2, 1, 1])
                    .map(|(path, count)| DirInfo {
                        path: path.to_string(),
                        count,
                    })
                    .collect(),
            }
        );
        let categories: Vec<(&str, usize)> =
            info.categories.iter().map(|c| (c.name, c.count)).collect();
        assert_eq!(
            categories,
            [("kick", 2), ("snare", 1), ("pad", 1), ("drums", 1)]
        );
    }

    /// A library of named kicks and closed and open hats, a hat that
    /// doesn't say which, like the closed ones, and a mystery sound like the
    /// open ones.
    fn kicks_and_hats() -> (Library, Random) {
        let mut random = Random(11);
        let (kick, closed, open) = (
            random.descriptor(),
            random.descriptor(),
            random.descriptor(),
        );
        let mut samples = Vec::new();
        for i in 0..8 {
            let d = random.near(&kick);
            samples.push(stored(i + 1, 1, &format!("/lib/Drums/Kick {i}.wav"), d));
            let d = random.near(&closed);
            samples.push(stored(
                i + 11,
                1,
                &format!("/lib/Drums/Closed Hat {i}.wav"),
                d,
            ));
            let d = random.near(&open);
            samples.push(stored(
                i + 21,
                1,
                &format!("/lib/Drums/Open Hat {i}.wav"),
                d,
            ));
        }
        let d = random.near(&closed);
        samples.push(stored(30, 1, "/lib/Drums/Hat.wav", d));
        samples.sort_by_key(|s| s.id);
        let d = random.near(&open);
        samples.push(stored(100, 3, "/lib/Misc/Mystery.wav", d));
        let mut folders = folders();
        folders.push(Folder {
            id: 3,
            path: "/lib/Misc".into(),
        });
        (Library::new(folders, samples), random)
    }

    #[test]
    fn guesses_categories_from_named_neighbours() {
        let (library, _) = kicks_and_hats();
        let guessed = library.guess(|| false).unwrap();
        let view = View::new(Arc::new(library), vec![Guess::default(); 26], 1)
            .with_guesses(guessed)
            .unwrap();
        assert_eq!(view.version, 2);
        let row = |path| view.row(view.library.find_path(path).unwrap(), None, &HashSet::new());
        let mystery = row("/lib/Misc/Mystery.wav");
        assert_eq!(
            (mystery.category, mystery.category_guessed),
            (Some("hihat"), true)
        );
        assert_eq!(
            (mystery.subcategory, mystery.subcategory_guessed),
            (Some("open"), true)
        );
        let hat = row("/lib/Drums/Hat.wav");
        assert_eq!(
            (
                hat.category_guessed,
                hat.subcategory,
                hat.subcategory_guessed
            ),
            (false, Some("closed"), true)
        );
        let open = row("/lib/Drums/Open Hat 3.wav");
        assert_eq!(
            (open.subcategory, open.subcategory_guessed),
            (Some("open"), false)
        );
        let count = |category: &str, subcategory: Option<&str>| {
            let q = query(|q| {
                q.category = Some(category.into());
                q.subcategory = subcategory.map(String::from);
            });
            view.search(&q, &HashSet::new(), None).positions.len()
        };
        assert_eq!(count("hihat", None), 18);
        assert_eq!(count("hihat", Some("open")), 9);
        assert_eq!(count("hihat", Some("pedal")), 0);
        let info = view.info(&HashSet::new());
        let hihat = info.categories.iter().find(|c| c.name == "hihat").unwrap();
        assert_eq!(
            hihat.subcategories,
            [
                SubcategoryInfo {
                    name: "closed",
                    count: 9
                },
                SubcategoryInfo {
                    name: "open",
                    count: 9
                },
            ]
        );
    }

    #[test]
    fn chosen_categories_beat_names() {
        let (library, _) = kicks_and_hats();
        let choose = |library: &Library, path: &str, category: &str, subcategory: Option<&str>| {
            let id = library.samples()[library.find_path(path).unwrap()].id;
            let chosen = Chosen {
                category: category.into(),
                subcategory: subcategory.map(String::from),
            };
            library.with_chosen(id, Some(chosen))
        };
        let kick = &library.samples()[library.find_path("/lib/Drums/Kick 3.wav").unwrap()];
        let stored_before = kick.stored(kick.folder_id);
        let library = choose(
            &library,
            "/lib/Drums/Kick 3.wav",
            "percussion",
            Some("shaker"),
        );
        let library = choose(&library, "/lib/Misc/Mystery.wav", "hihat", None);
        // A scan that stored it before it was chosen doesn't unchoose it.
        let library = library.with_changes(
            library.folders.clone(),
            vec![stored_before],
            &HashSet::new(),
        );
        let guessed = library.guess(|| false).unwrap();
        let view = View::new(Arc::new(library), vec![Guess::default(); 26], 1)
            .with_guesses(guessed)
            .unwrap();
        let row = |path| view.row(view.library.find_path(path).unwrap(), None, &HashSet::new());
        let kick = row("/lib/Drums/Kick 3.wav");
        assert_eq!(
            (
                kick.category,
                kick.chosen.is_some(),
                kick.subcategory,
                kick.subcategory_guessed
            ),
            (Some("percussion"), true, Some("shaker"), false)
        );
        // The kind is left to the sound.
        let mystery = row("/lib/Misc/Mystery.wav");
        assert_eq!(
            (
                mystery.category,
                mystery.category_guessed,
                mystery.subcategory,
                mystery.subcategory_guessed
            ),
            (Some("hihat"), false, Some("open"), true)
        );
        // Kept as it was chosen when the sample is read again.
        let s = &view.library.samples()[view.library.find_path("/lib/Drums/Kick 3.wav").unwrap()];
        assert_eq!(
            s.stored(s.folder_id).chosen,
            Some(Chosen {
                category: "percussion".into(),
                subcategory: Some("shaker".into())
            })
        );
        assert!(row("/lib/Drums/Kick 2.wav").chosen.is_none());
    }

    #[test]
    fn a_chosen_category_reads_the_name_for_itself() {
        let folder = Folder {
            id: 1,
            path: "/lib".into(),
        };
        let sample = |path: &str, category: Option<&str>| {
            let mut s = stored(1, 1, path, Random(1).descriptor());
            s.pitch = Some(40.0);
            s.chosen = category.map(|c| Chosen {
                category: c.into(),
                subcategory: None,
            });
            Sample::new(s, &folder)
        };
        // In a drum folder, a note in a name is a variation's. A bass's is
        // its own.
        assert_eq!(sample("/lib/Drums/Sample C2.wav", None).pitch, Some(40.0));
        assert_eq!(
            sample("/lib/Drums/Sample C2.wav", Some("bass")).pitch,
            Some(36.0)
        );
        // Any kind of hihat: the kind the name says.
        let open = sample("/lib/Misc/Open 2.wav", Some("hihat"));
        let category = open.category.unwrap();
        assert_eq!(names::category_name(category), "hihat");
        assert_eq!(
            open.subcategory
                .map(|t| names::subcategory_name(category, t)),
            Some("open")
        );
    }

    #[test]
    fn carries_guesses_to_the_next_library() {
        let (library, mut random) = kicks_and_hats();
        let guessed = library.guess(|| false).unwrap();
        let view = View::new(Arc::new(library), vec![Guess::default(); 26], 1)
            .with_guesses(guessed)
            .unwrap();
        let d = random.descriptor();
        let newcomer = stored(200, 3, "/lib/Misc/Newcomer.wav", d);
        let next = view.library.with_changes(
            view.library.folders.clone(),
            vec![newcomer],
            &HashSet::from([1]),
        );
        let next = view.carry(Arc::new(next));
        assert_eq!(next.version, 3);
        assert_eq!(next.library.len(), 26);
        let category = |path: &str| {
            let row = next.row(next.library.find_path(path).unwrap(), None, &HashSet::new());
            row.category
        };
        assert_eq!(category("/lib/Misc/Mystery.wav"), Some("hihat"));
        assert_eq!(category("/lib/Misc/Newcomer.wav"), None);
    }

    #[test]
    fn changes_keep_ids_and_follow_folders() {
        let f = fixture();
        let lib = &f.view.library;
        let mut random = Random(3);
        // Re-analyzed: the same id, read again.
        let mut again = stored(PAD, 2, "/lib/Synths/Pad F#min.wav", random.descriptor());
        again.duration = 9.0;
        let next = lib.with_changes(folders(), vec![again], &HashSet::from([KICK_SOFT]));
        assert_eq!(next.len(), 4);
        assert_eq!(next.get(PAD).unwrap().duration, 9.0);
        assert!(next.get(KICK_SOFT).is_none());

        // "/lib" absorbs both folders: its samples move to it, and read as
        // part of it.
        let lib_folder = Folder {
            id: 9,
            path: "/lib".into(),
        };
        let absorbed = next.with_changes(vec![lib_folder], Vec::new(), &HashSet::new());
        assert_eq!(absorbed.len(), 4);
        let pad = absorbed.get(PAD).unwrap();
        assert_eq!((pad.folder_id, pad.dir.as_str()), (9, "Synths"));
        // A folder that leaves takes its samples with it.
        let gone = absorbed.with_changes(Vec::new(), Vec::new(), &HashSet::new());
        assert!(gone.is_empty());
    }

    #[test]
    fn reads_guests_as_if_their_folder_were_the_librarys() {
        let a = Analyzed {
            duration: 0.5,
            sample_rate: 48000,
            channels: 1,
            bits: Some(16),
            pitch: Some(40.2),
            key: Some((1, false)),
            tempo: Some(96.0),
            looped: false,
            descriptor: Random(1).descriptor(),
            waveform: Vec::new(),
        };
        assert_eq!(
            guest_row(-1, "/elsewhere/One Shots/Bass C2.wav", &a),
            Row {
                id: -1,
                name: "Bass C2.wav".into(),
                path: "/elsewhere/One Shots/Bass C2.wav".into(),
                folder: "One Shots".into(),
                duration: 0.5,
                sample_rate: 48000,
                channels: 1,
                bits: Some(16),
                category: Some("bass"),
                category_guessed: false,
                chosen: None,
                subcategory: None,
                subcategory_guessed: false,
                looped: false,
                note: Some("C2".into()),
                key: None,
                tempo: None,
                favorite: false,
                similarity: None,
            }
        );
        // Drums get the pitch they sound at, and no key.
        let kick = guest_row(-2, "/elsewhere/Kick Db A1.wav", &a);
        assert_eq!(
            (kick.category, kick.note.as_deref(), kick.key),
            (Some("kick"), Some("E2"), None)
        );
        let drums = guest_row(-3, "/elsewhere/Drum Loop Fm.wav", &a);
        assert_eq!(
            (
                drums.category,
                drums.looped,
                drums.note,
                drums.key,
                drums.tempo
            ),
            (Some("drums"), true, None, None, Some(96.0))
        );
    }

    #[test]
    fn sorts_numbers_naturally() {
        let natural = |a: &str, b: &str| natural_key(a).cmp(&natural_key(b));
        use std::cmp::Ordering::*;
        assert_eq!(natural("Kick 2", "Kick 10"), Less);
        assert_eq!(natural("kick 02", "Kick 2"), Equal);
        assert_eq!(natural("Snare", "kick"), Greater);
        assert_eq!(natural("Kick", "Kick 1"), Less);
        assert_eq!(natural("Kick 9", "Kick-9"), Less);
        assert_eq!(natural("Kick 9", "Kicka"), Less);
        assert_eq!(natural("a1b", "a01c"), Less);
    }
}
