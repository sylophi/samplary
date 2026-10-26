//! What a sample's file and folder names say about it.

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    OneShot,
    Loop,
}

#[derive(Debug, Default, PartialEq)]
pub struct NameInfo {
    pub category: Option<&'static str>,
    /// One of the category's `subcategory_names`.
    pub subcategory: Option<&'static str>,
    pub kind: Option<Kind>,
    pub bpm: Option<f32>,
    /// Pitch class (0 = C) and whether the key is minor.
    pub key: Option<(usize, bool)>,
    /// MIDI note from tokens like "C3" or "F#2".
    pub note: Option<f32>,
}

/// Categories in priority order: when one name mentions several, the
/// earlier tier wins (a "Kick Bass" is a kick, a "Snare Loop" a snare).
#[rustfmt::skip]
const CATEGORIES: &[(&str, u8, &[&str])] = &[
    ("kick", 0, &["kick", "kicks", "kik", "kck", "bd", "bdrum", "bassdrum", "bassdrums"]),
    ("snare", 0, &["snare", "snares", "snr", "sd", "sn"]),
    ("clap", 0, &["clap", "claps", "clp", "handclap", "handclaps", "snap", "snaps", "fingersnap"]),
    ("hihat", 0, &["hat", "hats", "hihat", "hihats", "hh", "chh", "ohh", "phh", "hhc", "hho", "closedhat", "openhat"]),
    ("tom", 0, &["tom", "toms", "tomtom", "floortom"]),
    ("cymbal", 0, &["cymbal", "cymbals", "cym", "cymb", "crash", "crashes", "ride", "rides", "splash", "china"]),
    ("percussion", 0, &[
        "perc", "percs", "percussion", "shaker", "shakers", "conga", "congas", "bongo", "bongos",
        "cowbell", "tamb", "tambourine", "tamborine", "rim", "rimshot", "sidestick", "clave",
        "claves", "woodblock", "triangle", "timbale", "timbales", "guiro", "cabasa", "maraca",
        "maracas", "agogo", "cajon", "djembe", "tabla", "udu", "cuica", "whistle",
    ]),
    ("bass", 1, &["bass", "basses", "bassline", "basslines", "sub", "subbass", "reese"]),
    ("lead", 1, &["lead", "leads"]),
    ("pad", 1, &["pad", "pads", "atmos", "atmosphere", "atmospheres", "drone", "drones", "texture", "textures", "ambient"]),
    ("pluck", 1, &["pluck", "plucks", "plucked"]),
    ("keys", 1, &["keys", "piano", "pianos", "rhodes", "epiano", "organ", "organs", "wurli", "wurlitzer", "clav", "clavinet", "keyboard"]),
    ("bell", 1, &["bell", "bells", "chime", "chimes", "glockenspiel", "glock", "marimba", "vibraphone", "vibes", "xylophone", "kalimba", "mallet", "mallets", "celesta"]),
    ("chord", 1, &["chord", "chords", "stab", "stabs"]),
    ("fx", 1, &[
        "fx", "sfx", "riser", "risers", "uplifter", "downlifter", "sweep", "sweeps", "impact",
        "impacts", "whoosh", "swoosh", "noise", "noises", "glitch", "glitches", "zap", "laser",
        "transition", "transitions", "foley", "boom", "booms", "drop", "drops",
    ]),
    ("vocal", 1, &["vocal", "vocals", "vox", "voice", "voices", "chant", "chants", "acapella", "adlib", "adlibs", "choir", "spoken"]),
    ("guitar", 1, &["guitar", "guitars", "gtr", "strum", "strums"]),
    ("strings", 1, &["strings", "string", "violin", "violins", "viola", "cello", "cellos", "orchestra", "orchestral", "pizzicato", "pizz", "harp"]),
    ("brass", 1, &["brass", "horn", "horns", "trumpet", "trumpets", "trombone", "sax", "saxophone", "tuba", "woodwind", "woodwinds", "flute", "flutes", "clarinet", "oboe"]),
    ("drums", 2, &["drum", "drums", "drumloop", "drumloops", "beat", "beats", "break", "breaks", "breakbeat", "breakbeats", "groove", "grooves", "top", "tops", "fill", "fills", "kit"]),
    ("synth", 3, &["synth", "synths", "arp", "arps", "sequence", "seq"]),
];

const ACOUSTIC: &[&str] = &["acoustic", "acoustics", "live", "real", "jazz", "rock"];
const ELECTRONIC: &[&str] = &[
    "electronic",
    "electro",
    "elec",
    "808",
    "909",
    "606",
    "707",
    "727",
    "techno",
    "house",
    "edm",
    "trance",
    "analog",
    "analogue",
    "synth",
    "synthetic",
    "modular",
    "digital",
];
const DISTORTED: &[&str] = &[
    "distorted",
    "distortion",
    "dist",
    "hardstyle",
    "gabber",
    "hardcore",
    "saturated",
    "overdrive",
    "overdriven",
    "crushed",
    "bitcrushed",
    "clipped",
    "industrial",
];

/// Kinds of sound, each with the words that name it.
type Kinds = &'static [(&'static str, &'static [&'static str])];

/// The kinds of sound within some categories. When a name mentions
/// several, the earlier listed wins.
#[rustfmt::skip]
const SUBCATEGORIES: &[(&str, Kinds)] = &[
    ("kick", &[
        ("808", &["808", "808s", "sub", "subs", "subkick", "subkicks"]),
        ("distorted", DISTORTED),
        ("acoustic", ACOUSTIC),
        ("electronic", ELECTRONIC),
    ]),
    ("snare", &[
        ("acoustic", ACOUSTIC),
        ("electronic", ELECTRONIC),
    ]),
    ("hihat", &[
        ("closed", &["closed", "closedhat", "cl", "ch", "chh", "hhc", "clsd", "tight"]),
        ("open", &["open", "openhat", "op", "oh", "ohh", "hho"]),
        ("pedal", &["pedal", "pedalhat", "ped", "pd", "pdl", "phh", "hhp", "foot"]),
    ]),
    ("tom", &[
        ("high", &["hi", "high", "ht", "hitom", "hightom"]),
        ("mid", &["mid", "md", "mt", "midtom", "med", "medium"]),
        ("low", &["lo", "low", "lt", "lotom", "lowtom", "floor", "floortom", "flr"]),
    ]),
    ("cymbal", &[
        ("crash", &["crash", "crashes", "clash"]),
        ("ride", &["ride", "rides"]),
        ("splash", &["splash"]),
        ("china", &["china", "chinese"]),
    ]),
    ("percussion", &[
        ("shaker", &["shaker", "shakers", "shake", "maraca", "maracas", "cabasa", "egg"]),
        ("tambourine", &["tamb", "tambourine", "tambourines", "tamborine", "tambo", "jingle"]),
        ("hand drum", &[
            "conga", "congas", "bongo", "bongos", "djembe", "tabla", "cajon", "udu", "cuica",
            "surdo", "darbuka", "doumbek",
        ]),
        ("cowbell", &["cowbell", "cowbells", "agogo"]),
        ("rim", &["rim", "rims", "rimshot", "rimshots", "sidestick", "sidesticks", "stick", "sticks", "crossstick"]),
        ("wood", &["clave", "claves", "woodblock", "woodblocks", "block", "blocks", "wood", "wooden", "castanet", "castanets"]),
    ]),
    ("bass", &[
        ("808", &["808", "808s"]),
        ("sub", &["sub", "subs", "subbass"]),
        ("acoustic", &["acoustic", "upright", "double", "doublebass", "contrabass"]),
        ("electric", &["electric", "ebass", "finger", "fingered", "fing", "slap", "picked", "fretless"]),
        ("synth", &[
            "synth", "synths", "reese", "acid", "303", "fm", "wobble", "moog", "analog", "analogue",
            "digital", "saw", "square",
        ]),
    ]),
    ("keys", &[
        ("electric piano", &["electricpiano", "elpiano", "electric", "epiano", "ep", "rhodes", "wurli", "wurlitzer"]),
        ("piano", &["piano", "pianos", "grand", "upright"]),
        ("organ", &["organ", "organs", "hammond", "farfisa"]),
        ("clav", &["clav", "clavinet", "clavi"]),
    ]),
    ("fx", &[
        ("riser", &["riser", "risers", "rise", "uplifter", "uplifters", "buildup", "upsweep"]),
        ("downlifter", &["downlifter", "downlifters", "downer", "fall", "falling", "downsweep"]),
        ("impact", &["impact", "impacts", "boom", "booms", "slam", "thud", "explosion"]),
        ("whoosh", &["whoosh", "whooshes", "woosh", "swoosh", "swish", "swipe", "sweep", "sweeps"]),
        ("noise", &["noise", "noises", "static", "hiss", "crackle"]),
        ("glitch", &["glitch", "glitches", "stutter", "zap", "laser", "bleep", "blip"]),
    ]),
    ("guitar", &[
        ("acoustic", &["acoustic", "nylon", "classical", "folk"]),
        ("electric", &[
            "electric", "elec", "distorted", "overdrive", "overdriven", "crunch", "fuzz", "strat",
            "tele", "telecaster", "lespaul",
        ]),
    ]),
    ("strings", &[
        ("violin", &["violin", "violins", "fiddle"]),
        ("viola", &["viola", "violas"]),
        ("cello", &["cello", "cellos", "violoncello"]),
        ("harp", &["harp", "harps"]),
        ("ensemble", &["ensemble", "orchestra", "orchestral", "section"]),
    ]),
];

fn subcategories(category: &str) -> Kinds {
    SUBCATEGORIES
        .iter()
        .find(|(c, _)| *c == category)
        .map_or(&[], |(_, s)| s)
}

pub fn subcategory_names(category: u16) -> impl Iterator<Item = &'static str> {
    subcategories(category_name(category))
        .iter()
        .map(|(name, _)| *name)
}

/// The subcategory's place in its category's `subcategory_names`.
pub fn subcategory_index(category: u16, name: &str) -> Option<u8> {
    subcategory_names(category)
        .position(|n| n == name)
        .map(|i| i as u8)
}

pub fn subcategory_name(category: u16, index: u8) -> &'static str {
    subcategories(category_name(category))[index as usize].0
}

/// Drum sounds rarely have a key, and their names are full of letters that
/// look like one.
pub fn is_drum(category: &str) -> bool {
    matches!(
        category,
        "kick" | "snare" | "clap" | "hihat" | "tom" | "cymbal" | "percussion" | "drums"
    )
}

pub fn category_names() -> impl Iterator<Item = &'static str> {
    CATEGORIES.iter().map(|(name, _, _)| *name)
}

/// The category's place in `category_names`.
pub fn category_index(name: &str) -> Option<u16> {
    CATEGORIES
        .iter()
        .position(|(n, _, _)| *n == name)
        .map(|i| i as u16)
}

pub fn category_name(index: u16) -> &'static str {
    CATEGORIES[index as usize].0
}

const LOOP_WORDS: &[&str] = &[
    "loop",
    "loops",
    "drumloop",
    "drumloops",
    "breakbeat",
    "breakbeats",
];
const ONESHOT_WORDS: &[&str] = &["oneshot", "oneshots", "shot", "shots", "hit", "hits"];

/// Lowercased words, also split at camelCase and letter/digit seams:
/// "OpenHat02" -> ["open", "hat", "02"].
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in s.split(|c: char| !c.is_alphanumeric()) {
        let chars: Vec<char> = part.chars().collect();
        let mut cur = String::new();
        for (i, &c) in chars.iter().enumerate() {
            if let Some(&prev) = i.checked_sub(1).and_then(|j| chars.get(j)) {
                let next = chars.get(i + 1);
                let seam = (prev.is_alphabetic() != c.is_alphabetic())
                    || (prev.is_lowercase() && c.is_uppercase())
                    || (prev.is_uppercase()
                        && c.is_uppercase()
                        && next.is_some_and(|n| n.is_lowercase()));
                if seam && !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            cur.extend(c.to_lowercase());
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

/// Words in their original case, split only at separators.
fn raw_tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '#' || c == '♯' || c == '♭'))
        .filter(|t| !t.is_empty())
}

/// The name in `names` that `words` mention, the lowest tier first, then
/// the earliest mentioned.
fn named_in<'a>(
    words: &[String],
    names: impl Iterator<Item = (&'static str, u8, &'a [&'a str])> + Clone,
) -> Option<&'static str> {
    let mut best: Option<(u8, usize, &'static str)> = None;
    for (i, w) in words.iter().enumerate() {
        // "Bass Drum", "Hi Hat", "Hand Clap": two words naming one thing.
        let bigram = words
            .get(i + 1)
            .filter(|n| {
                w.len() > 1 && n.len() > 1 && w.chars().chain(n.chars()).all(char::is_alphabetic)
            })
            .map(|n| format!("{w}{n}"));
        for (name, tier, keys) in names.clone() {
            let hit =
                keys.contains(&w.as_str()) || bigram.as_deref().is_some_and(|b| keys.contains(&b));
            if hit && best.is_none_or(|(t, j, _)| (tier, i) < (t, j)) {
                best = Some((tier, i, name));
            }
        }
    }
    best.map(|(_, _, name)| name)
}

fn category_in(words: &[String]) -> Option<&'static str> {
    named_in(words, CATEGORIES.iter().copied())
}

fn subcategory_in(category: &str, words: &[String]) -> Option<&'static str> {
    let names = subcategories(category).iter().enumerate();
    named_in(words, names.map(|(i, &(name, keys))| (name, i as u8, keys)))
}

fn kind_in(words: &[String]) -> Option<Kind> {
    for (i, w) in words.iter().enumerate() {
        if LOOP_WORDS.contains(&w.as_str()) {
            return Some(Kind::Loop);
        }
        let one_shot = ONESHOT_WORDS.contains(&w.as_str())
            || (w == "one"
                && words
                    .get(i + 1)
                    .is_some_and(|n| n == "shot" || n == "shots"));
        if one_shot {
            return Some(Kind::OneShot);
        }
    }
    None
}

fn bpm_in(words: &[String]) -> Option<f32> {
    let plausible = |w: &String| w.parse::<f32>().ok().filter(|b| (40.0..=250.0).contains(b));
    let i = words.iter().position(|w| w == "bpm")?;
    i.checked_sub(1)
        .and_then(|j| plausible(&words[j]))
        .or_else(|| words.get(i + 1).and_then(plausible))
}

fn pitch_class(letter: char, accidental: &str) -> Option<usize> {
    let base = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let shift = match accidental {
        "" => 0,
        "#" | "♯" | "sharp" => 1,
        "b" | "♭" | "flat" => -1,
        _ => return None,
    };
    Some((base + shift + 12) as usize % 12)
}

/// Splits "F#min" into ('F', "#", "min").
fn split_note(token: &str) -> Option<(char, &str, &str)> {
    let letter = token.chars().next()?;
    let rest = &token[letter.len_utf8()..];
    let acc_len = ["sharp", "flat", "#", "♯", "♭", "b"]
        .iter()
        .find(|a| rest.starts_with(**a))
        .map_or(0, |a| a.len());
    Some((letter, &rest[..acc_len], &rest[acc_len..]))
}

fn key_in(name: &str) -> Option<(usize, bool)> {
    let tokens: Vec<&str> = raw_tokens(name).collect();
    for (i, t) in tokens.iter().enumerate() {
        let Some((letter, acc, mut quality)) = split_note(t) else {
            continue;
        };
        let next = tokens.get(i + 1).map(|n| n.to_ascii_lowercase());
        if quality.is_empty()
            && let Some(q) = next
                .as_deref()
                .filter(|n| matches!(*n, "min" | "minor" | "maj" | "major"))
        {
            quality = if q.starts_with("min") { "min" } else { "maj" };
        }
        let minor = match quality {
            "m" | "min" | "minor" => true,
            "" | "maj" | "major" => false,
            _ => continue,
        };
        let after_key = i > 0 && tokens[i - 1].eq_ignore_ascii_case("key");
        let spelled_out = matches!(quality, "min" | "minor" | "maj" | "major");
        let decorated = !acc.is_empty() || !quality.is_empty();
        // A bare "C" or a lowercase "am" is too often something else.
        let trusted = after_key || spelled_out || (letter.is_ascii_uppercase() && decorated);
        if trusted && let Some(pc) = pitch_class(letter, acc) {
            return Some((pc, minor));
        }
    }
    None
}

fn note_in(name: &str) -> Option<f32> {
    raw_tokens(name).find_map(|t| {
        let (letter, acc, octave) = split_note(t)?;
        if !letter.is_ascii_uppercase() || octave.len() != 1 {
            return None;
        }
        let octave: i32 = octave.parse().ok()?;
        let pc = pitch_class(letter, acc)? as i32;
        Some(((octave + 1) * 12 + pc) as f32)
    })
}

/// `folders` run from the library root down to the file's own folder.
pub fn parse(stem: &str, folders: &[&str]) -> NameInfo {
    parse_as(stem, folders, None)
}

/// As `parse`, for a sample known to be a `category`: the kind of it the
/// name says.
pub fn parse_as(stem: &str, folders: &[&str], category: Option<&'static str>) -> NameInfo {
    let own = words(stem);
    let folder_words: Vec<Vec<String>> = folders.iter().rev().map(|f| words(f)).collect();
    let category = category
        .or_else(|| category_in(&own))
        .or_else(|| folder_words.iter().find_map(|w| category_in(w)));
    let subcategory = category.and_then(|c| {
        subcategory_in(c, &own).or_else(|| folder_words.iter().find_map(|w| subcategory_in(c, w)))
    });
    let bpm = bpm_in(&own).or_else(|| folder_words.iter().find_map(|w| bpm_in(w)));
    let note = note_in(stem);
    // A note and no tempo names one sample of a playable instrument.
    let kind = kind_in(&own)
        .or(note.filter(|_| bpm.is_none()).map(|_| Kind::OneShot))
        .or_else(|| folder_words.iter().find_map(|w| kind_in(w)));
    NameInfo {
        category,
        subcategory,
        kind,
        bpm,
        key: key_in(stem),
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words() {
        assert_eq!(words("OpenHat02"), ["open", "hat", "02"]);
        assert_eq!(words("HHOpen_808"), ["hh", "open", "808"]);
        assert_eq!(
            words("TR909 SNARE-3.wav"),
            ["tr", "909", "snare", "3", "wav"]
        );
    }

    #[test]
    fn reads_categories() {
        assert_eq!(parse("Kick_Hard_01", &[]).category, Some("kick"));
        assert_eq!(parse("808 Bass Drum", &[]).category, Some("kick"));
        assert_eq!(parse("Hi Hat closed", &[]).category, Some("hihat"));
        assert_eq!(parse("Snare Loop 120bpm", &[]).category, Some("snare"));
        assert_eq!(parse("Reese C", &["Bass"]).category, Some("bass"));
        assert_eq!(
            parse("Sample 01", &["Pack", "Claps"]).category,
            Some("clap")
        );
        assert_eq!(parse("Sample 01", &["Pack"]).category, None);
        assert_eq!(parse("Groove 3", &["Loops"]).category, Some("drums"));
    }

    #[test]
    fn reads_subcategories() {
        let sub = |stem, folders| parse(stem, folders).subcategory;
        assert_eq!(sub("CYCdh_K1close_ClHat-09", &[]), Some("closed"));
        assert_eq!(sub("Cm_Hat_Op", &[]), Some("open"));
        assert_eq!(sub("Hh Pdl", &[]), Some("pedal"));
        assert_eq!(sub("Kick 01", &["Kit 4 - Electro"]), Some("electronic"));
        assert_eq!(sub("Kick 01", &["Roland TR-808"]), Some("808"));
        assert_eq!(sub("Distorted 909 Kick", &[]), Some("distorted"));
        assert_eq!(sub("Snare 01", &["Roland TR-808"]), Some("electronic"));
        assert_eq!(sub("Tom Room Hi", &[]), Some("high"));
        assert_eq!(sub("Floor Tom", &[]), Some("low"));
        assert_eq!(sub("Electric Piano C3", &[]), Some("electric piano"));
        assert_eq!(sub("BB_EBass_Fing_125-C1", &["Bass"]), Some("electric"));
        assert_eq!(sub("Pad 01", &[]), None);
    }

    #[test]
    fn reads_kind() {
        assert_eq!(parse("Drum Loop 01", &[]).kind, Some(Kind::Loop));
        assert_eq!(parse("Piano One Shot C3", &[]).kind, Some(Kind::OneShot));
        assert_eq!(parse("Pad", &["Loops"]).kind, Some(Kind::Loop));
        assert_eq!(parse("Bass 03 C1", &["Multis"]).kind, Some(Kind::OneShot));
        assert_eq!(parse("Acid C2 (120BPM)", &[]).kind, None);
        let take = parse("BB_EBass_Fing_125-C1", &["Bass", "125bpm"]);
        assert_eq!((take.kind, take.bpm), (None, Some(125.0)));
    }

    #[test]
    fn reads_bpm() {
        assert_eq!(parse("Loop_128bpm_Am", &[]).bpm, Some(128.0));
        assert_eq!(parse("Loop 90 BPM", &[]).bpm, Some(90.0));
        assert_eq!(parse("BPM_174_break", &[]).bpm, Some(174.0));
        assert_eq!(parse("Kick 120", &[]).bpm, None);
    }

    #[test]
    fn reads_keys() {
        assert_eq!(parse("Loop_128bpm_Am", &[]).key, Some((9, true)));
        assert_eq!(parse("Pad F#min", &[]).key, Some((6, true)));
        assert_eq!(parse("Chord Bb", &[]).key, Some((10, false)));
        assert_eq!(parse("Stab_Ebm_02", &[]).key, Some((3, true)));
        assert_eq!(parse("Bass key C", &[]).key, Some((0, false)));
        assert_eq!(parse("Kick A", &[]).key, None);
        assert_eq!(parse("Snare_D", &[]).key, None);
        assert_eq!(parse("FM Bass", &[]).key, None);
        assert_eq!(parse("Strings E minor", &[]).key, Some((4, true)));
    }

    #[test]
    fn reads_notes() {
        assert_eq!(parse("Piano_C3", &[]).note, Some(48.0));
        assert_eq!(parse("Bass F#1 long", &[]).note, Some(30.0));
        assert_eq!(parse("Kick 01", &[]).note, None);
    }
}
