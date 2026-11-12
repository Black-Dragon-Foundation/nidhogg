//! Font-spec records and the matching a native font driver needs, platform neutral.
//!
//! C reads a font spec into an [`NhgFontSpec`] (names are interned ids, see [`Names`]) and gets [`NhgFontEntity`]
//! records back.  What the platform supplies is only the list of faces ([`FontSource`]: Core Text on macOS, a fixture in
//! tests); the matching is here and follows `macfont.m`'s `macfont_list` and `macfont_match`:
//!
//! - **registry:** `iso10646-1`, `ascii-0`, `unicode-bmp` and none match every face; another registry needs a face
//!   that names it (`macfont_create_attributes_with_spec` refuses a registry it has no charset for);
//! - **family:** the spec's family and its aliases, in that order; no family is every face, in the source's order;
//! - **script and language:** a face must cover the spec's `:script` and `:lang`;
//! - **spacing:** a spec spacing of 0 or more keeps only proportional (below `FONT_SPACING_MONO`) or monospaced faces,
//!   unless the traits are synthesised;
//! - **colour fonts** are skipped unless the family is named or the script is `emoji`;
//! - **weight and slant:** a spec weight or slant that is unset (-1) or the synthetic one (200, `FONT_WEIGHT_SYNTHETIC_BOLD`
//!   and `FONT_SLANT_SYNTHETIC_ITALIC`) asks for a synthetic bold or italic; a face that lacks the trait is listed with
//!   it (`NHG_FS_SYNTH_*` in the entity) when no other face of the family is closer to the traits wanted
//!   (`macfont_closest_traits_index_p`).  A bitmap-only face is never synthesised bold.
//!
//! [`best`] is `macfont_match`: the face nearest to the spec, by slant, weight and width in that order, after the family
//! rank.  [`weight_to_trait`] and friends are the numeric maps of `macfont_create_attributes_with_spec`.

use std::collections::HashMap;
use std::ffi::c_char;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// A number the spec leaves unset.
pub const NHG_FS_UNSET: i32 = -1;
/// `:otf` features a record holds.
pub const NHG_FS_OTF_MAX: usize = 8;
/// `FONT_SPACING_*`.
pub const NHG_FS_SPACING_PROPORTIONAL: i32 = 0;
pub const NHG_FS_SPACING_DUAL: i32 = 90;
pub const NHG_FS_SPACING_MONO: i32 = 100;
pub const NHG_FS_SPACING_CHARCELL: i32 = 110;
/// `FONT_WEIGHT_SYNTHETIC_BOLD` and `FONT_SLANT_SYNTHETIC_ITALIC`.
pub const NHG_FS_SYNTHETIC_BOLD: i32 = 200;
pub const NHG_FS_SYNTHETIC_ITALIC: i32 = 200;
/// `NhgFontEntity::synth`: what the face is made to be.
pub const NHG_FS_SYNTH_ITALIC: u32 = 1;
pub const NHG_FS_SYNTH_BOLD: u32 = 2;
pub const NHG_FS_SYNTH_MONO: u32 = 4;

/// A font spec as C reads it.  Names are ids from [`Names`] (0: unset); sizes are device pixels (0: unset); the
/// numbers are `FONT_*_NUMERIC` (-1: unset).  Layout equals `NhgFontSpec`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NhgFontSpec {
    pub family: u32,
    pub foundry: u32,
    pub registry: u32,
    pub script: u32,
    pub language: u32,
    pub weight: i32,
    pub slant: i32,
    pub width: i32,
    pub spacing: i32,
    pub size: f32,
    /// How many of `otf` are set.
    pub otf_n: u32,
    pub otf: [u32; NHG_FS_OTF_MAX],
    /// An open font whose cascade picks a familyless spec's preferred family (0: none); listing only.
    pub cascade: u64,
}

impl Default for NhgFontSpec {
    fn default() -> Self {
        Self {
            family: 0,
            foundry: 0,
            registry: 0,
            script: 0,
            language: 0,
            weight: NHG_FS_UNSET,
            slant: NHG_FS_UNSET,
            width: NHG_FS_UNSET,
            spacing: NHG_FS_UNSET,
            size: 0.0,
            otf_n: 0,
            otf: [0; NHG_FS_OTF_MAX],
            cascade: 0,
        }
    }
}

/// A font the source lists: the spec's fields, filled in, a face id the driver opens it by, and what is synthesised.
/// Layout equals `NhgFontEntity`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NhgFontEntity {
    pub spec: NhgFontSpec,
    pub font_id: u64,
    pub synth: u32,
}

/// The names C and Rust share: a name has one id for the life of the process.
#[derive(Debug, Default)]
pub struct Names {
    ids: HashMap<String, u32>,
    names: Vec<String>,
}

impl Names {
    /// The id of `name` (made on the first call); `""` is 0, unset.
    pub fn intern(&mut self, name: &str) -> u32 {
        if name.is_empty() {
            return 0;
        }
        if let Some(id) = self.ids.get(name) {
            return *id;
        }
        self.names.push(name.to_owned());
        let id = self.names.len() as u32;
        self.ids.insert(name.to_owned(), id);
        id
    }

    /// The name of `id`; unset or unknown is `""`.
    pub fn name(&self, id: u32) -> &str {
        match id.checked_sub(1) {
            Some(i) => self.names.get(i as usize).map_or("", String::as_str),
            None => "",
        }
    }
}

/// A spec with its names resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct FontSpec {
    pub family: String,
    pub foundry: String,
    pub registry: String,
    pub script: String,
    pub language: String,
    pub weight: i32,
    pub slant: i32,
    pub width: i32,
    pub spacing: i32,
    pub size: f32,
    pub otf: Vec<String>,
    /// The open font whose own cascade picks the preferred family (`NhgFontSpec::cascade`; 0: none).
    pub cascade: u64,
}

impl FontSpec {
    pub fn from_record(r: &NhgFontSpec, names: &Names) -> Self {
        Self {
            family: names.name(r.family).to_owned(),
            foundry: names.name(r.foundry).to_owned(),
            registry: names.name(r.registry).to_owned(),
            script: names.name(r.script).to_owned(),
            language: names.name(r.language).to_owned(),
            weight: r.weight,
            slant: r.slant,
            width: r.width,
            spacing: r.spacing,
            size: r.size,
            otf: r.otf[..(r.otf_n as usize).min(NHG_FS_OTF_MAX)]
                .iter()
                .map(|id| names.name(*id).to_owned())
                .collect(),
            cascade: r.cascade,
        }
    }

    pub fn to_record(&self, names: &mut Names) -> NhgFontSpec {
        let mut otf = [0; NHG_FS_OTF_MAX];
        let n = self.otf.len().min(NHG_FS_OTF_MAX);
        for (slot, f) in otf.iter_mut().zip(&self.otf) {
            *slot = names.intern(f);
        }
        NhgFontSpec {
            family: names.intern(&self.family),
            foundry: names.intern(&self.foundry),
            registry: names.intern(&self.registry),
            script: names.intern(&self.script),
            language: names.intern(&self.language),
            weight: self.weight,
            slant: self.slant,
            width: self.width,
            spacing: self.spacing,
            size: self.size,
            otf_n: n as u32,
            otf,
            cascade: self.cascade,
        }
    }
}

/// One face a [`FontSource`] lists.
#[derive(Debug, Clone, PartialEq)]
pub struct FontFace {
    /// What the driver opens the face by.
    pub id: u64,
    pub family: String,
    pub foundry: String,
    pub weight: i32,
    pub slant: i32,
    pub width: i32,
    /// `NHG_FS_SPACING_*` of the face.
    pub spacing: i32,
    pub scripts: Vec<String>,
    pub languages: Vec<String>,
    /// Registries beyond Unicode the face covers.
    pub registries: Vec<String>,
    pub color: bool,
    pub bitmap_only: bool,
}

impl FontFace {
    fn mono(&self) -> bool {
        self.spacing >= NHG_FS_SPACING_MONO
    }
    fn bold(&self) -> bool {
        self.weight >= NHG_FS_SYNTHETIC_BOLD
    }
    fn italic(&self) -> bool {
        self.slant >= NHG_FS_SYNTHETIC_ITALIC
    }
    fn traits(&self) -> u32 {
        (u32::from(self.italic()) * NHG_FS_SYNTH_ITALIC)
            | (u32::from(self.bold()) * NHG_FS_SYNTH_BOLD)
            | (u32::from(self.mono()) * NHG_FS_SYNTH_MONO)
    }
}

/// Where the faces come from: Core Text on macOS, or a fixture in the tests.
pub trait FontSource {
    /// Every face, grouped by family in the order the platform prefers.
    fn faces(&self) -> Vec<FontFace>;
    /// The faces of `family` when `faces` leaves it out but the platform opens it by name (`macfont_list` matches a
    /// named family directly); none by default.
    fn named(&self, _family: &str) -> Vec<FontFace> {
        Vec::new()
    }
    /// The families `family` stands for (`monospace`, `sans-serif`), most wanted first.
    fn aliases(&self, _family: &str) -> Vec<String> {
        Vec::new()
    }
    /// The family the platform draws `script`'s characters in when no font is named, the text in `language` (empty:
    /// none; `mac_font_create_preferred_family_for_attributes` lays the characters out with the spec's first
    /// language): `macfont_list`'s preferred family, moved to the front of an unnamed list; none by default.
    fn preferred(&self, _script: &str, _language: &str) -> Option<String> {
        None
    }
    /// As `preferred`, from the open font `font`'s own cascade (a face in that font falling back: Core Text's
    /// `CTFontCreateForString`); by default the font changes nothing.
    fn preferred_from(&self, _font: u64, script: &str, language: &str) -> Option<String> {
        self.preferred(script, language)
    }
    /// Whether `face` has `script` and `language` (either empty: any); by default from the face's own lists.  A source
    /// that reads coverage lazily (Core Text's character sets) answers here instead.
    fn fits(&self, face: &FontFace, script: &str, language: &str) -> bool {
        covers(&face.scripts, script) && covers(&face.languages, language)
    }
    /// Whether face `id` has character `c` (`macfont_has_char` on an entity); `None`: not known without opening it.
    fn has_char(&self, _id: u64, _c: u32) -> Option<bool> {
        None
    }
    /// The family the platform draws `chars` in for `language` (empty: none) when no font is named: a registry's
    /// preferred family, as `preferred` is a script's; none by default.
    fn preferred_chars(&self, _chars: &[u32], _language: &str) -> Option<String> {
        None
    }
}

/// A listed font: the spec's fields as the face has them.
#[derive(Debug, Clone, PartialEq)]
pub struct FontEntity {
    pub face: FontFace,
    pub synth: u32,
    pub size: f32,
}

impl FontEntity {
    pub fn to_record(&self, names: &mut Names) -> NhgFontEntity {
        let spec = FontSpec {
            family: self.face.family.clone(),
            foundry: self.face.foundry.clone(),
            registry: "iso10646-1".into(),
            script: String::new(),
            language: String::new(),
            weight: self.face.weight,
            slant: self.face.slant,
            width: self.face.width,
            spacing: self.face.spacing,
            size: self.size,
            otf: Vec::new(),
            cascade: 0,
        };
        NhgFontEntity {
            spec: spec.to_record(names),
            font_id: self.face.id,
            synth: self.synth,
        }
    }
}

fn unicode_registry(r: &str) -> bool {
    r.is_empty() || matches!(r, "iso10646-1" | "ascii-0" | "unicode-bmp")
}

/// A registry's characters and language (macfont's `cf_charset_table`): a face has the registry when it has every one
/// of the characters.
struct Charset {
    name: &'static str,
    chars: &'static [u32],
    language: &'static str,
}

#[rustfmt::skip]
static CHARSETS: &[Charset] = &[
    Charset { name: "iso8859-1", chars: &[0x00A0, 0x00A1, 0x00B4, 0x00BC, 0x00D0], language: "" },
    Charset { name: "iso8859-2", chars: &[0x00A0, 0x010E], language: "" },
    Charset { name: "iso8859-3", chars: &[0x00A0, 0x0108], language: "" },
    Charset { name: "iso8859-4", chars: &[0x00A0, 0x00AF, 0x0128, 0x0156, 0x02C7], language: "" },
    Charset { name: "iso8859-5", chars: &[0x00A0, 0x0401], language: "" },
    Charset { name: "iso8859-6", chars: &[0x00A0, 0x060C], language: "" },
    Charset { name: "iso8859-7", chars: &[0x00A0, 0x0384], language: "" },
    Charset { name: "iso8859-8", chars: &[0x00A0, 0x05D0], language: "" },
    Charset { name: "iso8859-9", chars: &[0x00A0, 0x00A1, 0x00BC, 0x011E], language: "" },
    Charset { name: "iso8859-10", chars: &[0x00A0, 0x00D0, 0x0128, 0x2015], language: "" },
    Charset { name: "iso8859-11", chars: &[0x00A0, 0x0E01], language: "" },
    Charset { name: "iso8859-13", chars: &[0x00A0, 0x201C], language: "" },
    Charset { name: "iso8859-14", chars: &[0x00A0, 0x0174], language: "" },
    Charset { name: "iso8859-15", chars: &[0x00A0, 0x00A1, 0x00D0, 0x0152], language: "" },
    Charset { name: "iso8859-16", chars: &[0x00A0, 0x0218], language: "" },
    Charset { name: "gb2312.1980-0", chars: &[0x4E13], language: "zh-Hans" },
    Charset { name: "big5-0", chars: &[0x4EDC], language: "zh-Hant" },
    Charset { name: "jisx0208.1983-0", chars: &[0x4E55], language: "ja" },
    Charset { name: "ksc5601.1987-0", chars: &[0xAC00], language: "ko" },
    Charset { name: "cns11643.1992-1", chars: &[0xFE32], language: "zh-Hant" },
    Charset { name: "cns11643.1992-2", chars: &[0x4E33, 0x7934], language: "" },
    Charset { name: "cns11643.1992-3", chars: &[0x201A9], language: "" },
    Charset { name: "cns11643.1992-4", chars: &[0x20057], language: "" },
    Charset { name: "cns11643.1992-5", chars: &[0x20000], language: "" },
    Charset { name: "cns11643.1992-6", chars: &[0x20003], language: "" },
    Charset { name: "cns11643.1992-7", chars: &[0x20055], language: "" },
    Charset { name: "gbk-0", chars: &[0x4E06], language: "zh-Hans" },
    Charset { name: "jisx0212.1990-0", chars: &[0x4E44], language: "" },
    Charset { name: "jisx0213.2000-1", chars: &[0xFA10], language: "ja" },
    Charset { name: "jisx0213.2000-2", chars: &[0xFA49], language: "" },
    Charset { name: "jisx0213.2004-1", chars: &[0x20B9F], language: "" },
    Charset { name: "viscii1.1-1", chars: &[0x1EA0, 0x1EAE, 0x1ED2], language: "vi" },
    Charset { name: "tis620.2529-1", chars: &[0x0E01], language: "th" },
    Charset { name: "windows-1251", chars: &[0x0401, 0x0490], language: "ru" },
    Charset { name: "koi8-r", chars: &[0x0401, 0x2219], language: "ru" },
    Charset { name: "mulelao-1", chars: &[0x0E81], language: "lo" },
    Charset { name: "unicode-sip", chars: &[0x20000], language: "" },
];

/// Whether `pattern` (`*` any run, `?` and `.` any character) matches somewhere in `s`, ignoring case
/// (`macfont_get_charset`'s unanchored regexp: `.` in a registry is literal there, so it is here too).
fn glob_in(pattern: &[u8], s: &[u8]) -> bool {
    fn at(p: &[u8], s: &[u8]) -> bool {
        match p.split_first() {
            None => true,
            Some((b'*', rest)) => (0..=s.len()).any(|i| at(rest, &s[i..])),
            Some((&c, rest)) => s.split_first().is_some_and(|(&d, srest)| {
                (c == b'?' || c.eq_ignore_ascii_case(&d)) && at(rest, srest)
            }),
        }
    }
    (0..=s.len()).any(|i| at(pattern, &s[i..]))
}

/// The first table charset `registry` matches.
fn registry_charset(registry: &str) -> Option<&'static Charset> {
    CHARSETS
        .iter()
        .find(|c| glob_in(registry.as_bytes(), c.name.as_bytes()))
}

/// The language a list asks for: a registry's own first, else the spec's (macfont's `languages`' first).
fn spec_language<'a>(spec: &'a FontSpec, charset: Option<&'static Charset>) -> &'a str {
    match charset {
        Some(c) if !c.language.is_empty() => c.language,
        _ => &spec.language,
    }
}

fn cjk(language: &str) -> bool {
    ["ja", "ko", "zh"].iter().any(|p| language.starts_with(p))
}

fn covers(list: &[String], want: &str) -> bool {
    want.is_empty() || list.iter().any(|s| s.eq_ignore_ascii_case(want))
}

/// The faces `spec` allows before weight and slant are looked at, in the order of their families.
fn candidates(src: &dyn FontSource, spec: &FontSpec) -> Vec<(usize, FontFace)> {
    let faces = src.faces();
    let mut order: Vec<(usize, FontFace)> = Vec::new();
    if spec.family.is_empty() {
        order.extend(faces.into_iter().map(|f| (0, f)));
    } else {
        let mut names = vec![spec.family.clone()];
        names.extend(src.aliases(&spec.family));
        for (rank, n) in names.iter().enumerate() {
            let before = order.len();
            order.extend(
                faces
                    .iter()
                    .filter(|f| f.family.eq_ignore_ascii_case(n))
                    .map(|f| (rank, f.clone())),
            );
            // a generic name (`monospace`) stands for listed families, and is never a family itself
            if order.len() == before && src.aliases(n).is_empty() {
                order.extend(src.named(n).into_iter().map(|f| (rank, f)));
            }
        }
    }
    let charset = if unicode_registry(&spec.registry) {
        None
    } else {
        registry_charset(&spec.registry)
    };
    let language = spec_language(spec, charset);
    // a registry's characters stand for the script (macfont looks at the script only without a charset)
    let script = if charset.is_some() { "" } else { &spec.script };
    let preferred = if !spec.family.is_empty() {
        None
    } else if let Some(c) = charset {
        src.preferred_chars(c.chars, language)
    } else if !script.is_empty() && spec.cascade != 0 {
        src.preferred_from(spec.cascade, script, language)
    } else if !script.is_empty() {
        src.preferred(script, language)
    } else {
        None
    };
    if let Some(p) = &preferred {
        // a stable move: the preferred family's faces first, in their order
        let (mut front, rest): (Vec<_>, Vec<_>) = order
            .into_iter()
            .partition(|(_, f)| f.family.eq_ignore_ascii_case(p));
        front.extend(rest);
        order = front;
    }
    let synth_mono = cjk(language) && (spec.spacing < 0 || spec.spacing == NHG_FS_SPACING_MONO);
    order.retain(|(_, f)| {
        (unicode_registry(&spec.registry)
            || covers(&f.registries, &spec.registry)
            || charset.is_some_and(|c| {
                c.chars
                    .iter()
                    .all(|&ch| src.has_char(f.id, ch) == Some(true))
            }))
            // the preferred family is Core Text's own pick for the language, so it needs only the script there:
            // Hiragino Sans lays kana out in Simplified Chinese though its languages name only Japanese
            && (src.fits(f, script, language)
                || preferred
                    .as_deref()
                    .is_some_and(|p| f.family.eq_ignore_ascii_case(p))
                    && src.fits(f, script, ""))
            && !(spec.spacing >= 0
                && !synth_mono
                && f.mono() != (spec.spacing >= NHG_FS_SPACING_MONO))
            && !(f.color && spec.family.is_empty() && spec.script != "emoji")
    });
    order
}

fn distance(a: u32, b: u32) -> u32 {
    ((a ^ b) & (NHG_FS_SYNTH_ITALIC | NHG_FS_SYNTH_BOLD | NHG_FS_SYNTH_MONO)).count_ones()
}

/// `macfont_list`: every font that fits `spec`, the faces in the source's order, a face's synthetic variants after it.
pub fn list(src: &dyn FontSource, spec: &FontSpec) -> Vec<FontEntity> {
    let found = candidates(src, spec);
    let charset = if unicode_registry(&spec.registry) {
        None
    } else {
        registry_charset(&spec.registry)
    };
    let synth_mono = cjk(spec_language(spec, charset))
        && (spec.spacing < 0 || spec.spacing == NHG_FS_SPACING_MONO);
    let want_italic = spec.slant < 0 || spec.slant == NHG_FS_SYNTHETIC_ITALIC;
    let want_bold = spec.weight < 0 || spec.weight == NHG_FS_SYNTHETIC_BOLD;
    let wanted = (u32::from(want_italic) * NHG_FS_SYNTH_ITALIC)
        | (u32::from(want_bold) * NHG_FS_SYNTH_BOLD)
        | (u32::from(synth_mono) * NHG_FS_SYNTH_MONO);
    let mut out = Vec::new();
    for (_, face) in &found {
        // the traits of the faces of the same family: a synthesised variant is only made by the closest one
        let same: Vec<u32> = found
            .iter()
            .filter(|(_, o)| o.family.eq_ignore_ascii_case(&face.family))
            .map(|(_, o)| o.traits())
            .collect();
        let own = face.traits();
        let mut min = (wanted ^ own) & (NHG_FS_SYNTH_ITALIC | NHG_FS_SYNTH_BOLD);
        if spec.slant < 0 {
            min &= !NHG_FS_SYNTH_ITALIC;
        }
        if spec.weight < 0 {
            min &= !NHG_FS_SYNTH_BOLD;
        }
        let mut max = wanted & !own;
        if face.bitmap_only && (min ^ max) & NHG_FS_SYNTH_BOLD != 0 {
            max &= !NHG_FS_SYNTH_BOLD;
        }
        if spec.spacing >= 0 {
            min |= max & NHG_FS_SYNTH_MONO;
        }
        for synth in 0..8u32 {
            // each bit runs from `min` to `max`
            if synth & min != min || synth & !(max | min) != 0 {
                continue;
            }
            let closest = same
                .iter()
                .all(|t| distance(own | synth, *t) >= distance(own | synth, own));
            if synth == 0 || closest {
                // the entity names only what the face lacks
                out.push(FontEntity {
                    face: face.clone(),
                    synth: synth & !own,
                    size: spec.size,
                });
            }
        }
    }
    out
}

/// `macfont_match`: the one font nearest to `spec` (family rank first, then slant, weight and width distance), or
/// none.  A face that fits the spec's registry, script, language and spacing is a candidate; ties keep the source's
/// order.
pub fn best(src: &dyn FontSource, spec: &FontSpec) -> Option<FontEntity> {
    let dist = |want: i32, have: i32| if want < 0 { 0 } else { (want - have).abs() };
    candidates(src, spec)
        .into_iter()
        .min_by_key(|(rank, f)| {
            (
                *rank,
                dist(spec.slant, f.slant),
                dist(spec.weight, f.weight),
                dist(spec.width, f.width),
            )
        })
        .map(|(_, face)| FontEntity {
            face,
            synth: 0,
            size: spec.size,
        })
}

/// The piecewise-linear maps of `macfont_create_attributes_with_spec`: points are (Core Text trait, Emacs numeric).
const WEIGHT_POINTS: &[(f64, f64)] = &[
    (-0.6, 0.0),
    (-0.4, 40.0),
    (-0.23, 50.0),
    (-0.115, 55.0),
    (0.0, 80.0),
    (0.2, 100.0),
    (0.3, 180.0),
    (0.4, 200.0),
    (0.6, 205.0),
    (0.8, 210.0),
    (1.0, 250.0),
];
const SLANT_POINTS: &[(f64, f64)] = &[(0.0, 100.0), (0.1, 200.0)];
const WIDTH_POINTS: &[(f64, f64)] = &[
    (-0.4, 50.0),
    (-0.3, 63.0),
    (-0.2, 75.0),
    (-0.1, 87.0),
    (0.0, 100.0),
    (0.1, 113.0),
    (0.2, 125.0),
    (0.3, 150.0),
    (0.4, 200.0),
];

/// Emacs numeric to Core Text trait: linear between the points, clamped at the ends.
fn to_trait(points: &[(f64, f64)], numeric: i32) -> f64 {
    let v = f64::from(numeric);
    let (first, last) = (points[0], points[points.len() - 1]);
    if v <= first.1 {
        return first.0;
    }
    if v >= last.1 {
        return last.0;
    }
    let i = points.windows(2).position(|w| v <= w[1].1).unwrap_or(0);
    let (a, b) = (points[i], points[i + 1]);
    a.0 + (b.0 - a.0) * (v - a.1) / (b.1 - a.1)
}

/// Core Text trait to Emacs numeric: the inverse, rounded.
fn from_trait(points: &[(f64, f64)], t: f64) -> i32 {
    let (first, last) = (points[0], points[points.len() - 1]);
    if t <= first.0 {
        return first.1 as i32;
    }
    if t >= last.0 {
        return last.1 as i32;
    }
    let i = points.windows(2).position(|w| t <= w[1].0).unwrap_or(0);
    let (a, b) = (points[i], points[i + 1]);
    (a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0)).round() as i32
}

/// `mac_font_descriptor_get_adjusted_weight`: a face whose weight trait is -0.40 (to the hundredth, Core Text's
/// ultra-light) is regular (0) when AppKit weighs it 5, its regular (`weightOfFont:`, asked only then): Core Text
/// reports some regular faces ultra-light (Arial Unicode MS since OS X 10.11).
pub fn adjusted_weight(t: f64, appkit_weight: impl FnOnce() -> Option<isize>) -> f64 {
    if (t * 100.0).round() as i64 == -40 && appkit_weight() == Some(5) {
        0.0
    } else {
        t
    }
}

pub fn weight_to_trait(weight: i32) -> f64 {
    to_trait(WEIGHT_POINTS, weight)
}
pub fn weight_from_trait(t: f64) -> i32 {
    from_trait(WEIGHT_POINTS, t)
}
pub fn slant_to_trait(slant: i32) -> f64 {
    to_trait(SLANT_POINTS, slant)
}
pub fn slant_from_trait(t: f64) -> i32 {
    from_trait(SLANT_POINTS, t)
}
pub fn width_to_trait(width: i32) -> f64 {
    to_trait(WIDTH_POINTS, width)
}
pub fn width_from_trait(t: f64) -> i32 {
    from_trait(WIDTH_POINTS, t)
}

// ---- The C entry points: one process-wide name table and font source -------------------------------------------------

fn names() -> MutexGuard<'static, Names> {
    static NAMES: OnceLock<Mutex<Names>> = OnceLock::new();
    NAMES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

type Source = Box<dyn FontSource + Send>;

fn source() -> MutexGuard<'static, Option<Source>> {
    static SOURCE: OnceLock<Mutex<Option<Source>>> = OnceLock::new();
    SOURCE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Install the font source `nhg_font_list` and `nhg_font_match` use (the driver does it once at start).
pub fn set_source(s: Source) {
    *source() = Some(s);
}

/// The name of id `id` (`""`: unset or unknown).
pub fn name_of(id: u32) -> String {
    names().name(id).to_owned()
}

/// The id of the name at `s` (`len` bytes, not NUL-terminated), made on the first call; 0 for an empty name or bad UTF-8.
pub(crate) fn intern_raw(s: *const c_char, len: usize) -> u32 {
    if s.is_null() {
        return 0;
    }
    // SAFETY: `s` holds `len` readable bytes (the caller's promise).
    let bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), len) };
    std::str::from_utf8(bytes).map_or(0, |n| names().intern(n))
}

/// The name of `id` into `buf` (`cap` bytes, NUL-terminated), its length into `*len`; false when `cap` is too small.
pub(crate) fn name_raw(id: u32, buf: *mut c_char, cap: usize, len: *mut usize) -> bool {
    let n = names();
    let name = n.name(id);
    if !len.is_null() {
        // SAFETY: non-null and writable (the caller's promise).
        unsafe { len.write(name.len()) };
    }
    if buf.is_null() || cap <= name.len() {
        return false;
    }
    // SAFETY: `buf` holds `cap` > `name.len()` writable bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(name.as_ptr(), buf.cast::<u8>(), name.len());
        buf.add(name.len()).write(0);
    }
    true
}

/// Whether face `id` has character `c`: 1 or 0, -1 when the source cannot tell (or there is none).
pub(crate) fn has_char(id: u64, c: u32) -> i32 {
    match source().as_deref().and_then(|s| s.has_char(id, c)) {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    }
}

/// Fill `out` (`cap` records) with the fonts for `spec`, and put how many there are into `*n`: more than `out` holds
/// when it is too small (the caller asks again with room for them all).  `all` is `nhg_font_list` (every font), else
/// `nhg_font_match` (the nearest one).  Returns false without a source.
pub(crate) fn run(spec: &NhgFontSpec, all: bool, out: &mut [NhgFontEntity], n: &mut usize) -> bool {
    let guard = source();
    let Some(src) = guard.as_deref() else {
        *n = 0;
        return false;
    };
    let spec = FontSpec::from_record(spec, &names());
    let found = if all {
        list(src, &spec)
    } else {
        best(src, &spec).into_iter().collect()
    };
    let mut nm = names();
    *n = found.len();
    for (slot, e) in out.iter_mut().zip(&found) {
        *slot = e.to_record(&mut nm);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        faces: Vec<FontFace>,
        /// Faces the listing leaves out, found by their family's name.
        hidden: Vec<FontFace>,
    }
    impl FontSource for Fixture {
        fn faces(&self) -> Vec<FontFace> {
            self.faces.clone()
        }
        fn named(&self, family: &str) -> Vec<FontFace> {
            self.hidden
                .iter()
                .filter(|f| f.family.eq_ignore_ascii_case(family))
                .cloned()
                .collect()
        }
        fn aliases(&self, family: &str) -> Vec<String> {
            match family {
                "monospace" => vec!["Menlo".into(), "Courier".into()],
                "sans-serif" => vec!["Helvetica".into()],
                _ => Vec::new(),
            }
        }
        // a latin face has the Latin letters, a han one the CJK ideographs; other faces are not known
        fn has_char(&self, id: u64, c: u32) -> Option<bool> {
            let f = self.faces.iter().find(|f| f.id == id)?;
            if covers(&f.scripts, "latin") {
                Some(c < 0x250)
            } else if covers(&f.scripts, "han") {
                Some((0x4e00..0xa000).contains(&c))
            } else {
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn face(
        id: u64,
        family: &str,
        weight: i32,
        slant: i32,
        spacing: i32,
        scripts: &[&str],
    ) -> FontFace {
        FontFace {
            id,
            family: family.into(),
            foundry: "apple".into(),
            weight,
            slant,
            width: 100,
            spacing,
            scripts: scripts.iter().map(|s| (*s).into()).collect(),
            languages: Vec::new(),
            registries: Vec::new(),
            color: false,
            bitmap_only: false,
        }
    }

    fn fixture() -> Fixture {
        let latin = &["latin"][..];
        let mut hira = face(9, "Hiragino", 80, 100, 0, &["han", "kana"]);
        hira.languages = vec!["ja".into()];
        let mut emoji = face(10, "Apple Color Emoji", 80, 100, 0, &["emoji"]);
        emoji.color = true;
        let mut bitmap = face(11, "Bits", 80, 100, 100, latin);
        bitmap.bitmap_only = true;
        let mut koi = face(12, "Koi", 80, 100, 0, latin);
        koi.registries = vec!["koi8-r".into()];
        koi.languages = vec!["ru".into()];
        Fixture {
            faces: vec![
                face(1, "Menlo", 80, 100, 100, latin),
                face(2, "Menlo", 200, 100, 100, latin),
                face(3, "Menlo", 80, 200, 100, latin),
                face(4, "Helvetica", 80, 100, 0, latin),
                face(5, "Helvetica", 200, 100, 0, latin),
                face(6, "Courier", 80, 100, 100, latin),
                face(7, "Times", 80, 100, 0, latin),
                face(8, "Symbols", 80, 100, 0, &["symbol"]),
                hira,
                emoji,
                bitmap,
                koi,
            ],
            hidden: vec![
                face(13, "Courier Hidden", 80, 100, 100, latin),
                face(14, "sans-serif", 80, 100, 0, latin),
            ],
        }
    }

    fn spec() -> FontSpec {
        FontSpec {
            family: String::new(),
            foundry: String::new(),
            registry: "iso10646-1".into(),
            script: String::new(),
            language: String::new(),
            weight: 80,
            slant: 100,
            width: NHG_FS_UNSET,
            spacing: NHG_FS_UNSET,
            size: 0.0,
            otf: Vec::new(),
            cascade: 0,
        }
    }

    fn ids(v: &[FontEntity]) -> Vec<(u64, u32)> {
        v.iter().map(|e| (e.face.id, e.synth)).collect()
    }

    #[test]
    fn record_round_trip() {
        let mut n = Names::default();
        let s = FontSpec {
            family: "Menlo".into(),
            foundry: "apple".into(),
            script: "latin".into(),
            language: "en".into(),
            weight: 200,
            slant: 100,
            width: 100,
            spacing: 100,
            size: 26.0,
            otf: vec!["liga".into(), "calt".into()],
            ..spec()
        };
        let r = s.to_record(&mut n);
        assert_eq!(r.otf_n, 2);
        assert_ne!(r.family, 0);
        assert_eq!(FontSpec::from_record(&r, &n), s);
        // an unset spec is all zeros and -1, and comes back unset
        let e = FontSpec {
            registry: String::new(),
            weight: -1,
            slant: -1,
            ..spec()
        };
        let r = e.to_record(&mut n);
        assert_eq!(
            (r.family, r.registry, r.weight, r.size, r.otf_n),
            (0, 0, -1, 0.0, 0)
        );
        assert_eq!(FontSpec::from_record(&r, &n), e);
        // the same name is the same id, and an unknown id is no name
        assert_eq!(n.intern("Menlo"), n.intern("Menlo"));
        assert_eq!(n.name(9999), "");
    }

    #[test]
    fn entity_record_carries_the_face() {
        let mut n = Names::default();
        let fx = fixture();
        let e = best(
            &fx,
            &FontSpec {
                family: "Menlo".into(),
                weight: 200,
                size: 24.0,
                ..spec()
            },
        )
        .unwrap();
        let r = e.to_record(&mut n);
        assert_eq!((r.font_id, r.spec.weight, r.spec.size), (2, 200, 24.0));
        assert_eq!(n.name(r.spec.family), "Menlo");
        assert_eq!(n.name(r.spec.registry), "iso10646-1");
    }

    #[test]
    fn weight_slant_width_maps() {
        // the points of macfont.m, and the interpolation between them
        assert_eq!(weight_to_trait(80), 0.0);
        assert_eq!(weight_to_trait(200), 0.4);
        assert_eq!(weight_to_trait(0), -0.6);
        assert_eq!(weight_to_trait(300), 1.0);
        assert!((weight_to_trait(140) - 0.25).abs() < 1e-9);
        assert_eq!(slant_to_trait(100), 0.0);
        assert!((slant_to_trait(200) - 0.1).abs() < 1e-9);
        assert_eq!(width_to_trait(100), 0.0);
        assert!((width_to_trait(75) + 0.2).abs() < 1e-9);
        for w in [0, 40, 50, 80, 100, 180, 200, 210, 250] {
            assert_eq!(weight_from_trait(weight_to_trait(w)), w, "weight {w}");
        }
        assert_eq!(slant_from_trait(0.1), 200);
        assert_eq!(width_from_trait(0.4), 200);
        assert_eq!(width_from_trait(-1.0), 50);
    }

    #[test]
    fn an_ultra_light_trait_that_appkit_weighs_regular_is_regular() {
        // Core Text's -0.40 with AppKit's 5 is regular (80); AppKit's own ultra-light (2) stays 40
        let w = |t: f64, appkit: Option<isize>| weight_from_trait(adjusted_weight(t, || appkit));
        assert_eq!(w(-0.4, Some(5)), 80);
        assert_eq!(w(-0.4001, Some(5)), 80);
        assert_eq!(w(-0.4, Some(2)), 40);
        assert_eq!(w(-0.4, None), 40);
        // AppKit is asked only for -0.40
        let asked = std::cell::Cell::new(false);
        let probe = |t: f64| {
            adjusted_weight(t, || {
                asked.set(true);
                Some(5)
            })
        };
        assert_eq!(probe(-0.23), -0.23);
        assert_eq!(probe(0.4), 0.4);
        assert!(!asked.get());
        assert_eq!(probe(-0.4), 0.0);
        assert!(asked.get());
    }

    #[test]
    fn family_left_out_of_the_listing_found_by_name() {
        // macfont_list matches a named family directly: Core Text hides some from its list (Courier on macOS 27)
        let fx = fixture();
        let got = list(
            &fx,
            &FontSpec {
                family: "courier hidden".into(),
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(13, 0)]);
        // a listed family does not ask
        let got = list(
            &fx,
            &FontSpec {
                family: "Courier".into(),
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(6, 0)]);
        // a generic name is not looked up by name: it stands for the families it lists
        let got = list(
            &fx,
            &FontSpec {
                family: "sans-serif".into(),
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(4, 0), (5, 0)]);
    }

    #[test]
    fn family_and_aliases() {
        let fx = fixture();
        let got = list(
            &fx,
            &FontSpec {
                family: "Helvetica".into(),
                ..spec()
            },
        );
        assert_eq!(
            ids(&got),
            [(4, 0), (5, 0)],
            "font.c ranks them by weight later"
        );
        // `monospace` is Menlo first, then Courier
        let got = list(
            &fx,
            &FontSpec {
                family: "monospace".into(),
                ..spec()
            },
        );
        assert_eq!(
            ids(&got).iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [1, 2, 3, 6]
        );
        // a family is case-insensitive; an unknown one lists nothing
        assert_eq!(
            list(
                &fx,
                &FontSpec {
                    family: "menlo".into(),
                    ..spec()
                }
            )
            .len(),
            3
        );
        assert!(
            list(
                &fx,
                &FontSpec {
                    family: "Nope".into(),
                    ..spec()
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn spacing_filter() {
        let fx = fixture();
        let fam = |sp: i32| {
            let got = list(
                &fx,
                &FontSpec {
                    spacing: sp,
                    script: "latin".into(),
                    ..spec()
                },
            );
            got.iter().map(|e| e.face.id).collect::<Vec<_>>()
        };
        assert_eq!(fam(NHG_FS_SPACING_MONO), [1, 2, 3, 6, 11]);
        assert_eq!(fam(NHG_FS_SPACING_CHARCELL), [1, 2, 3, 6, 11]);
        assert_eq!(fam(NHG_FS_SPACING_PROPORTIONAL), [4, 5, 7, 12]);
        assert_eq!(fam(NHG_FS_SPACING_DUAL), [4, 5, 7, 12]);
        assert_eq!(fam(NHG_FS_UNSET).len(), 9);
        // a CJK language lets the spacing through: the monospace is made
        let ja = list(
            &fx,
            &FontSpec {
                spacing: NHG_FS_SPACING_MONO,
                language: "ja".into(),
                ..spec()
            },
        );
        assert_eq!(ids(&ja), [(9, NHG_FS_SYNTH_MONO)]);
    }

    #[test]
    fn script_coverage() {
        let fx = fixture();
        let names = |s: &str| {
            list(
                &fx,
                &FontSpec {
                    script: s.into(),
                    ..spec()
                },
            )
            .iter()
            .map(|e| e.face.id)
            .collect::<Vec<_>>()
        };
        assert_eq!(names("symbol"), [8]);
        assert_eq!(names("kana"), [9]);
        assert!(names("klingon").is_empty());
        // a language narrows the same way
        let ja = list(
            &fx,
            &FontSpec {
                language: "ja".into(),
                ..spec()
            },
        );
        // (a CJK language with the spacing open also offers the face made monospace)
        assert_eq!(ids(&ja), [(9, 0), (9, NHG_FS_SYNTH_MONO)]);
    }

    /// A source with a preferred family per script and coverage it answers itself.
    struct Preferring(Fixture);
    impl FontSource for Preferring {
        fn faces(&self) -> Vec<FontFace> {
            self.0.faces()
        }
        fn preferred(&self, script: &str, _language: &str) -> Option<String> {
            (script == "latin").then(|| "Times".into())
        }
        fn fits(&self, face: &FontFace, script: &str, _language: &str) -> bool {
            // "greek" is had by Times and Helvetica only, though their lists don't say so
            if script == "greek" {
                return face.family == "Times" || face.family == "Helvetica";
            }
            covers(&face.scripts, script)
        }
    }

    #[test]
    fn the_preferred_family_comes_first_and_sources_answer_coverage() {
        let px = Preferring(fixture());
        let got = |s: &str| {
            list(
                &px,
                &FontSpec {
                    script: s.into(),
                    ..spec()
                },
            )
        };
        let latin: Vec<u64> = got("latin").iter().map(|e| e.face.id).collect();
        assert_eq!(latin.first(), Some(&7));
        assert_eq!(latin.iter().filter(|&&i| i == 7).count(), 1);
        // the others keep their order
        assert_eq!(&latin[1..4], &[1, 2, 3]);
        assert_eq!(
            got("greek").iter().map(|e| e.face.id).collect::<Vec<_>>(),
            [4, 5, 7]
        );
        // a named family is not reordered
        let named = list(
            &px,
            &FontSpec {
                family: "Menlo".into(),
                script: "latin".into(),
                ..spec()
            },
        );
        assert_eq!(named[0].face.id, 1);
    }

    /// A source whose preferred family for Han text depends on its language, as Core Text's cascade does.
    struct ByLanguage(Vec<FontFace>);
    impl FontSource for ByLanguage {
        fn faces(&self) -> Vec<FontFace> {
            self.0.clone()
        }
        fn preferred(&self, script: &str, language: &str) -> Option<String> {
            match (script, language) {
                ("han", "ja") => Some("Hiragino".into()),
                ("han", "zh-Hans") => Some("PingFang SC".into()),
                _ => None,
            }
        }
    }

    #[test]
    fn the_preferred_family_follows_the_spec_language() {
        // both faces have Han and both languages, so only the preferred family orders them
        let langs = || vec!["ja".to_string(), "zh-Hans".to_string()];
        let mut pingfang = face(13, "PingFang SC", 80, 100, 0, &["han"]);
        pingfang.languages = langs();
        let mut hira = face(9, "Hiragino", 80, 100, 0, &["han"]);
        hira.languages = langs();
        let src = ByLanguage(vec![pingfang, hira]);
        let first = |lang: &str| {
            list(
                &src,
                &FontSpec {
                    script: "han".into(),
                    language: lang.into(),
                    ..spec()
                },
            )
            .first()
            .map(|e| e.face.family.clone())
        };
        assert_eq!(first("ja").as_deref(), Some("Hiragino"));
        assert_eq!(first("zh-Hans").as_deref(), Some("PingFang SC"));
        // no language: the source's own order
        assert_eq!(first("").as_deref(), Some("PingFang SC"));
    }

    /// A source whose preferred family for kana in Simplified Chinese is a Japanese family, as Core Text's is.
    struct KanaInChinese(Vec<FontFace>);
    impl FontSource for KanaInChinese {
        fn faces(&self) -> Vec<FontFace> {
            self.0.clone()
        }
        fn preferred(&self, script: &str, language: &str) -> Option<String> {
            (script == "kana" && language == "zh-Hans").then(|| "Hiragino Sans".into())
        }
    }

    #[test]
    fn the_preferred_family_is_listed_though_its_languages_lack_the_spec_s() {
        let mut arial = face(3, "Arial Unicode MS", 80, 100, 0, &["kana"]);
        arial.languages = vec!["ja".to_string(), "zh-Hans".to_string()];
        let mut hira = face(9, "Hiragino Sans", 80, 100, 0, &["kana"]);
        hira.languages = vec!["ja".to_string()];
        let mut other = face(11, "Osaka", 80, 100, 0, &["kana"]);
        other.languages = vec!["ja".to_string()];
        let src = KanaInChinese(vec![arial, hira, other]);
        // each family once (a CJK language adds a synthetic monospaced variant after each face)
        let families = |lang: &str| -> Vec<String> {
            let mut f: Vec<String> = list(
                &src,
                &FontSpec {
                    script: "kana".into(),
                    language: lang.into(),
                    ..spec()
                },
            )
            .into_iter()
            .map(|e| e.face.family)
            .collect();
            f.dedup();
            f
        };
        // Core Text's pick first; another face without the language stays out
        assert_eq!(families("zh-Hans"), ["Hiragino Sans", "Arial Unicode MS"]);
        assert_eq!(
            families("ja"),
            ["Arial Unicode MS", "Hiragino Sans", "Osaka"]
        );
    }

    /// A source whose system preferred family for Hebrew differs from a font's own cascade, as Core Text's does
    /// (Lucida Grande, but Courier New from Menlo).
    struct FromFont(Vec<FontFace>);
    impl FontSource for FromFont {
        fn faces(&self) -> Vec<FontFace> {
            self.0.clone()
        }
        fn preferred(&self, script: &str, _language: &str) -> Option<String> {
            (script == "hebrew").then(|| "Lucida Grande".into())
        }
        fn preferred_from(&self, font: u64, script: &str, language: &str) -> Option<String> {
            if font == 7 && script == "hebrew" {
                Some("Courier New".into())
            } else {
                self.preferred(script, language)
            }
        }
    }

    #[test]
    fn a_cascade_font_s_own_pick_comes_first() {
        let src = FromFont(vec![
            face(1, "Courier New", 80, 100, 0, &["hebrew"]),
            face(2, "Lucida Grande", 80, 100, 0, &["hebrew"]),
        ]);
        let first = |cascade: u64, family: &str| {
            list(
                &src,
                &FontSpec {
                    family: family.into(),
                    script: "hebrew".into(),
                    cascade,
                    ..spec()
                },
            )
            .first()
            .map(|e| e.face.family.clone())
        };
        // no cascade font: the system's pick; from font 7: its cascade's
        assert_eq!(first(0, "").as_deref(), Some("Lucida Grande"));
        assert_eq!(first(7, "").as_deref(), Some("Courier New"));
        // a named family is not reordered
        assert_eq!(first(7, "Lucida Grande").as_deref(), Some("Lucida Grande"));
    }

    #[test]
    fn registry_iso10646_and_others() {
        let fx = fixture();
        let n = |r: &str| {
            list(
                &fx,
                &FontSpec {
                    registry: r.into(),
                    script: "latin".into(),
                    ..spec()
                },
            )
            .len()
        };
        assert_eq!(n("iso10646-1"), n(""));
        assert_eq!(n("ascii-0"), n(""));
        assert_eq!(n("unicode-bmp"), n(""));
        let koi = list(
            &fx,
            &FontSpec {
                registry: "koi8-r".into(),
                script: "latin".into(),
                ..spec()
            },
        );
        assert_eq!(koi.iter().map(|e| e.face.id).collect::<Vec<_>>(), [12]);
        assert!(
            list(
                &fx,
                &FontSpec {
                    registry: "iso8859-99".into(),
                    ..spec()
                }
            )
            .is_empty()
        );
    }

    /// A source with CJK faces that answers coverage by character and a preferred family per registry.
    struct Cjk(Vec<FontFace>);
    impl FontSource for Cjk {
        fn faces(&self) -> Vec<FontFace> {
            self.0.clone()
        }
        fn has_char(&self, id: u64, c: u32) -> Option<bool> {
            let f = self.0.iter().find(|f| f.id == id)?;
            Some(
                (covers(&f.scripts, "han") && (0x4e00..0xa000).contains(&c))
                    || (covers(&f.scripts, "hangul") && (0xac00..0xd7a4).contains(&c)),
            )
        }
        fn preferred_chars(&self, chars: &[u32], language: &str) -> Option<String> {
            match (chars, language) {
                ([0x4E13], "zh-Hans") => Some("PingFang SC".into()),
                ([0xAC00], "ko") => Some("Gothic".into()),
                _ => None,
            }
        }
    }

    #[test]
    fn a_registry_is_its_characters_and_language_and_has_a_preferred_family() {
        let mut fx = fixture().faces;
        let mut song = face(15, "Song", 80, 100, 0, &["han"]);
        song.languages = vec!["zh-Hans".into()];
        let mut pingfang = face(13, "PingFang SC", 80, 100, 0, &["han"]);
        pingfang.languages = vec!["zh-Hans".into()];
        let mut gothic = face(14, "Gothic", 80, 100, 0, &["han", "hangul"]);
        gothic.languages = vec!["ko".into()];
        fx.extend([song, pingfang, gothic]);
        let src = Cjk(fx);
        let ids = |r: &str| {
            let mut v: Vec<u64> = list(
                &src,
                &FontSpec {
                    registry: r.into(),
                    ..spec()
                },
            )
            .iter()
            .map(|e| e.face.id)
            .collect();
            v.dedup();
            v
        };
        // Chinese faces only (Hiragino is Japanese, Gothic Korean), the preferred one first
        assert_eq!(ids("gb2312.1980-0"), [13, 15]);
        assert_eq!(ids("GB2312.1980-*"), [13, 15]);
        assert_eq!(ids("ksc5601.1987-0"), [14]);
        // the registry's language makes a CJK face monospace as well
        let ko = list(
            &src,
            &FontSpec {
                registry: "ksc5601.1987-0".into(),
                ..spec()
            },
        );
        assert!(ko.iter().any(|e| e.synth & NHG_FS_SYNTH_MONO != 0));
    }

    #[test]
    fn colour_fonts_need_a_name_or_the_emoji_script() {
        let fx = fixture();
        assert!(!list(&fx, &spec()).iter().any(|e| e.face.id == 10));
        let by_name = list(
            &fx,
            &FontSpec {
                family: "Apple Color Emoji".into(),
                ..spec()
            },
        );
        assert_eq!(by_name.len(), 1);
        let by_script = list(
            &fx,
            &FontSpec {
                script: "emoji".into(),
                ..spec()
            },
        );
        assert_eq!(
            by_script.iter().map(|e| e.face.id).collect::<Vec<_>>(),
            [10]
        );
    }

    #[test]
    fn synthetic_bold_and_italic() {
        let fx = fixture();
        // Times has only a regular face: a bold spec lists it with synthetic bold
        let got = list(
            &fx,
            &FontSpec {
                family: "Times".into(),
                weight: 200,
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(7, NHG_FS_SYNTH_BOLD)]);
        // an unset weight lists the face as it is and with synthetic bold
        let got = list(
            &fx,
            &FontSpec {
                family: "Times".into(),
                weight: -1,
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(7, 0), (7, NHG_FS_SYNTH_BOLD)]);
        // Helvetica has a bold face of its own: the regular one is not made bold, and the real bold is listed whole
        let got = list(
            &fx,
            &FontSpec {
                family: "Helvetica".into(),
                weight: 200,
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(5, 0)]);
        // italic and bold together on a face with neither
        let got = list(
            &fx,
            &FontSpec {
                family: "Times".into(),
                weight: 200,
                slant: 200,
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(7, NHG_FS_SYNTH_ITALIC | NHG_FS_SYNTH_BOLD)]);
        // Menlo, weight unset: its regular and bold faces as they are, and its italic one as it is and made bold
        let got = list(
            &fx,
            &FontSpec {
                family: "Menlo".into(),
                weight: -1,
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(1, 0), (2, 0), (3, 0), (3, NHG_FS_SYNTH_BOLD)]);
        // a bitmap-only face is not made bold when the weight is open, though an explicit bold spec still gets it
        let got = list(
            &fx,
            &FontSpec {
                family: "Bits".into(),
                weight: -1,
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(11, 0)]);
        let got = list(
            &fx,
            &FontSpec {
                family: "Bits".into(),
                weight: 200,
                ..spec()
            },
        );
        assert_eq!(ids(&got), [(11, NHG_FS_SYNTH_BOLD)]);
    }

    #[test]
    fn match_takes_the_nearest() {
        let fx = fixture();
        let m = |s: FontSpec| best(&fx, &s).map(|e| e.face.id);
        assert_eq!(
            m(FontSpec {
                family: "Menlo".into(),
                weight: 200,
                ..spec()
            }),
            Some(2)
        );
        assert_eq!(
            m(FontSpec {
                family: "Menlo".into(),
                weight: 180,
                ..spec()
            }),
            Some(2)
        );
        assert_eq!(
            m(FontSpec {
                family: "Menlo".into(),
                weight: 90,
                ..spec()
            }),
            Some(1)
        );
        // slant counts before weight
        assert_eq!(
            m(FontSpec {
                family: "Menlo".into(),
                weight: 200,
                slant: 200,
                ..spec()
            }),
            Some(3)
        );
        // an alias: the first family that exists wins
        assert_eq!(
            m(FontSpec {
                family: "monospace".into(),
                ..spec()
            }),
            Some(1)
        );
        assert_eq!(
            m(FontSpec {
                family: "sans-serif".into(),
                weight: 200,
                ..spec()
            }),
            Some(5)
        );
        // monospace only
        assert_eq!(
            m(FontSpec {
                spacing: 100,
                script: "latin".into(),
                ..spec()
            }),
            Some(1)
        );
        assert_eq!(
            m(FontSpec {
                family: "Nope".into(),
                ..spec()
            }),
            None
        );
    }

    #[test]
    fn entry_points_use_the_source() {
        set_source(Box::new(fixture()));
        let s = FontSpec {
            family: "Menlo".into(),
            weight: 200,
            ..spec()
        };
        let rec = s.to_record(&mut names());
        let mut out = [NhgFontEntity::default(); 4];
        let mut n = 0;
        assert!(run(&rec, false, &mut out, &mut n));
        assert_eq!((n, out[0].font_id), (1, 2));
        let open = FontSpec {
            family: "Menlo".into(),
            weight: -1,
            ..spec()
        }
        .to_record(&mut names());
        assert!(run(&open, true, &mut out, &mut n));
        assert_eq!(n, 4); // Menlo's three faces and the italic one made bold
        // more fonts than `out` holds: the count says so, and the first ones are written
        let mut two = [NhgFontEntity::default(); 2];
        assert!(run(&open, true, &mut two, &mut n));
        assert_eq!(
            (n, two[0].font_id, two[1].font_id),
            (4, out[0].font_id, out[1].font_id)
        );
        // a face's characters, without opening it
        assert_eq!(has_char(1, 'a' as u32), 1);
        assert_eq!(has_char(1, 0x5b57), 0);
        assert_eq!(has_char(9, 0x5b57), 1);
        assert_eq!(has_char(8, 'a' as u32), -1);
        assert_eq!(has_char(99, 'a' as u32), -1);
        let mut buf = [0 as c_char; 16];
        let mut len = 0;
        assert!(name_raw(
            out[0].spec.family,
            buf.as_mut_ptr(),
            buf.len(),
            &mut len
        ));
        assert_eq!(len, 5);
        let id = intern_raw(c"Menlo".as_ptr(), 5);
        assert_eq!(id, out[0].spec.family);
        assert!(!name_raw(id, buf.as_mut_ptr(), 3, &mut len)); // too small
    }
}
