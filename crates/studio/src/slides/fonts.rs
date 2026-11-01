//! The system's fonts, loaded once (in the background, the first time a
//! slide is drawn) and shared by the text layout and the SVG renderers.
//! A font a deck names that this system lacks is drawn with a look-alike
//! (Linux has Liberation or DejaVu where macOS and Windows have Arial).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use cosmic_text::FontSystem;

pub struct Fonts {
    pub system: Mutex<FontSystem>,
    pub db: Arc<fontdb::Database>,
    /// Lowercase family names this system has.
    families: HashMap<String, String>,
    /// The weight drawn for a family, bold or not (see [`Fonts::weight`]).
    weights: Mutex<HashMap<(String, bool), u16>>,
}

/// Look-alikes for the families decks name most, best first.
const ALIKE: &[(&[&str], &[&str])] = &[
    (
        &[
            "arial",
            "helvetica",
            "helvetica neue",
            "calibri",
            "aptos",
            "segoe ui",
            "inter",
            "verdana",
            "tahoma",
            "trebuchet ms",
            "gill sans",
        ],
        &[
            "Arial",
            "Helvetica Neue",
            "Helvetica",
            "Liberation Sans",
            "Arimo",
            "DejaVu Sans",
            "Noto Sans",
            "Segoe UI",
        ],
    ),
    (
        &["arial black", "impact"],
        &["Arial Black", "Arial", "Liberation Sans", "DejaVu Sans"],
    ),
    (
        &[
            "georgia",
            "times new roman",
            "times",
            "cambria",
            "garamond",
            "palatino",
        ],
        &[
            "Georgia",
            "Times New Roman",
            "Times",
            "Liberation Serif",
            "Tinos",
            "DejaVu Serif",
            "Noto Serif",
        ],
    ),
    (
        &[
            "courier new",
            "courier",
            "consolas",
            "menlo",
            "monaco",
            "sf mono",
            "cascadia code",
            "jetbrains mono",
        ],
        &[
            "Menlo",
            "Consolas",
            "Courier New",
            "Liberation Mono",
            "Cousine",
            "DejaVu Sans Mono",
            "Noto Sans Mono",
        ],
    ),
];

/// The fonts, loading them first if no one has.
pub fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(load)
}

fn load() -> Fonts {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut families = HashMap::new();
    for face in db.faces() {
        for (name, _) in &face.families {
            families
                .entry(name.to_lowercase())
                .or_insert_with(|| name.clone());
        }
    }
    let first = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| families.get(&name.to_lowercase()).cloned())
    };
    if let Some(sans) = first(ALIKE[0].1) {
        db.set_sans_serif_family(sans);
    }
    if let Some(serif) = first(ALIKE[2].1) {
        db.set_serif_family(serif);
    }
    if let Some(mono) = first(ALIKE[3].1) {
        db.set_monospace_family(mono);
    }
    let locale = sys_locale().unwrap_or_else(|| "en-US".into());
    let shared = Arc::new(db.clone());
    Fonts {
        system: Mutex::new(FontSystem::new_with_locale_and_db(locale, db)),
        db: shared,
        families,
        weights: Mutex::default(),
    }
}

fn sys_locale() -> Option<String> {
    std::env::var("LANG")
        .ok()
        .and_then(|lang| lang.split('.').next().map(|tag| tag.replace('_', "-")))
        .filter(|tag| !tag.is_empty() && tag != "C" && tag != "POSIX")
}

impl Fonts {
    /// The family to draw `name` with here: itself when installed, else
    /// its closest look-alike, else the sans-serif default.
    pub fn family(&self, name: &str) -> String {
        let lower = name.trim().to_lowercase();
        if let Some(found) = self.families.get(&lower) {
            return found.clone();
        }
        let group = ALIKE
            .iter()
            .find(|(names, _)| names.contains(&lower.as_str()))
            .map(|(_, alikes)| *alikes)
            .unwrap_or(ALIKE[0].1);
        group
            .iter()
            .find_map(|alike| self.families.get(&alike.to_lowercase()).cloned())
            .unwrap_or_else(|| "sans-serif".into())
    }

    /// Every family this system has, sorted (for the font menu).
    /// The weight to ask for when drawing `family` (as [`Fonts::family`]
    /// names it) bold or not: the nearest it has. A family of one heavy
    /// face (Arial Black) is that weight either way, so the layout and the
    /// drawing pick the same face.
    pub fn weight(&self, family: &str, bold: bool) -> u16 {
        let wanted: u16 = if bold { 700 } else { 400 };
        let key = (family.to_lowercase(), bold);
        let mut weights = self
            .weights
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        *weights.entry(key).or_insert_with(|| {
            self.db
                .faces()
                .filter(|face| {
                    face.families
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case(family))
                })
                .map(|face| face.weight.0)
                .min_by_key(|weight| (weight.abs_diff(wanted), *weight))
                .unwrap_or(wanted)
        })
    }

    /// Whether this system has `name`.
    pub fn has(&self, name: &str) -> bool {
        self.families.contains_key(&name.trim().to_lowercase())
    }

    pub fn all(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .families
            .values()
            .filter(|name| !name.starts_with('.'))
            .cloned()
            .collect();
        names.sort_by_key(|name| name.to_lowercase());
        names
    }
}
