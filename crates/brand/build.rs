//! Reads `brand/brand.toml` at the repository root, checks it, and writes it
//! out as Rust constants (`$OUT_DIR/brand.rs`, included by `src/lib.rs`). A
//! mistake in the file stops the build with a message saying what is wrong,
//! rather than shipping an app that cannot find its data or its updates.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Brand {
    app: App,
    /// Features switched on or off by key; a missing key means on.
    #[serde(default)]
    features: std::collections::BTreeMap<String, bool>,
    links: Links,
    updates: Updates,
}

/// The keys `[features]` may switch, as `ph_brand::Feature::key` names them
/// (a test in `src/lib.rs` keeps the two lists the same).
const FEATURES: &[&str] = &[
    "files",
    "changes",
    "pulls",
    "board",
    "browser",
    "devices",
    "runs",
    "tests",
    "terminal",
    "logcat",
    "github",
    "schedules",
    "utilities",
    "usage",
    "network_drives",
    "parallel_runs",
    "presentations",
    "chrome_extension",
    "quick_chat",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct App {
    name: String,
    vendor: String,
    id: String,
    data_folder: String,
    keychain: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Links {
    repository: String,
    releases: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Updates {
    enabled: bool,
    key: String,
    #[serde(default)]
    sources: Vec<Source>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    name: String,
    manifest: String,
    files: String,
}

fn main() {
    let folder = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../brand");
    let file = folder.join("brand.toml");
    println!("cargo:rerun-if-changed={}", file.display());
    let text = std::fs::read_to_string(&file)
        .unwrap_or_else(|err| fail(&format!("cannot read {}: {err}", file.display())));
    let brand: Brand = toml::from_str(&text)
        .unwrap_or_else(|err| fail(&format!("{} is not valid: {err}", file.display())));
    let key = check(&brand, &folder);
    println!("cargo:rerun-if-changed={}", key.display());
    for icon in ICONS {
        let path = folder.join("icons").join(icon);
        println!("cargo:rerun-if-changed={}", path.display());
        if !path.is_file() {
            fail(&format!("brand/icons/{icon} is missing"));
        }
    }

    let mut out = String::new();
    custom_icons(&folder.join("icons/custom"), &mut out);
    for key in brand.features.keys() {
        if !FEATURES.contains(&key.as_str()) {
            fail(&format!(
                "[features] has no feature `{key}` (it has: {})",
                FEATURES.join(", ")
            ));
        }
    }
    let off: Vec<&str> = brand
        .features
        .iter()
        .filter(|(_, on)| !**on)
        .map(|(key, _)| key.as_str())
        .collect();
    writeln!(
        out,
        "/// The features `[features]` switches off.\nconst SWITCHED_OFF: &[&str] = &{off:?};\n/// Every key `[features]` takes.\n#[cfg(test)]\nconst FEATURE_KEYS: &[&str] = &{FEATURES:?};"
    )
    .unwrap();
    let app = &brand.app;
    let constant = |out: &mut String, doc: &str, name: &str, value: &str| {
        writeln!(out, "/// {doc}\npub const {name}: &str = {value:?};").unwrap();
    };
    constant(
        &mut out,
        "The app's name, as people see it.",
        "NAME",
        &app.name,
    );
    constant(&mut out, "Who makes this build.", "VENDOR", &app.vendor);
    constant(
        &mut out,
        "Reverse-DNS id (the macOS bundle identifier).",
        "ID",
        &app.id,
    );
    constant(
        &mut out,
        "The folder, in the system's app data folder, for this build's data.",
        "DATA_FOLDER",
        &app.data_folder,
    );
    constant(
        &mut out,
        "The name secrets are filed under in the system's keychain.",
        "KEYCHAIN",
        &app.keychain,
    );
    constant(
        &mut out,
        "The project's page.",
        "REPOSITORY_URL",
        &brand.links.repository,
    );
    constant(
        &mut out,
        "The page of the newest release.",
        "RELEASES_URL",
        &brand.links.releases,
    );
    writeln!(
        out,
        "/// Whether this build looks for updates at all.\npub const UPDATES_ENABLED: bool = {};",
        brand.updates.enabled
    )
    .unwrap();
    writeln!(
        out,
        "/// Where updates come from, tried in order.\npub const UPDATE_SOURCES: &[UpdateSource] = &["
    )
    .unwrap();
    for source in &brand.updates.sources {
        writeln!(
            out,
            "    UpdateSource {{ name: {:?}, manifest: {:?}, files: {:?} }},",
            source.name, source.manifest, source.files
        )
        .unwrap();
    }
    writeln!(out, "];").unwrap();
    writeln!(
        out,
        "/// The public half of the key that signs `latest.json` (Ed25519, base64).\npub const UPDATE_KEY: &str = include_str!({:?});",
        key.display().to_string()
    )
    .unwrap();
    // The name without spaces, as file names and User-Agents take it (the
    // packaging scripts make the same: scripts/package/brand.sh stem).
    let stem: String = app.name.chars().filter(|c| *c != ' ').collect();
    constant(
        &mut out,
        "The name without spaces: file names, the User-Agent.",
        "STEM",
        &stem,
    );
    writeln!(
        out,
        "/// [`STEM`] as a string literal, for `concat!`.\n#[macro_export]\nmacro_rules! stem {{\n    () => {{\n        {stem:?}\n    }};\n}}"
    )
    .unwrap();
    // The name as a literal, for `concat!` in strings that must be
    // `&'static str` (MCP tool descriptions).
    writeln!(
        out,
        "/// The app's name as a string literal, for `concat!`.\n#[macro_export]\nmacro_rules! name {{\n    () => {{\n        {:?}\n    }};\n}}",
        app.name
    )
    .unwrap();

    let target = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("brand.rs");
    std::fs::write(&target, out).unwrap();
}

/// The icons `src/lib.rs`, `crates/app` (Windows) and the packaging scripts
/// expect in `brand/icons` (`scripts/package/icon.py` makes them all from
/// one picture).
const ICONS: &[&str] = &[
    "app-1024.png",
    "app-256.png",
    "title-bar.png",
    "app.ico",
    "app.icns",
];

/// An organisation's own icons, `brand/icons/custom/<name>.svg`, embedded
/// as `CUSTOM_ICONS` (name, SVG); the app serves each at
/// `icons/custom/<name>.svg`.
fn custom_icons(folder: &Path, out: &mut String) {
    println!("cargo:rerun-if-changed={}", folder.display());
    let mut icons: Vec<(String, PathBuf)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_string();
            Some((name, path))
        })
        .collect();
    icons.sort();
    writeln!(
        out,
        "/// An organisation's own icons (`brand/icons/custom/<name>.svg`): name, SVG.\npub const CUSTOM_ICONS: &[(&str, &[u8])] = &["
    )
    .unwrap();
    for (name, path) in icons {
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            fail(&format!(
                "brand/icons/custom/{name}.svg: icon names are lowercase letters, digits, - and _"
            ));
        }
        writeln!(
            out,
            "    ({name:?}, include_bytes!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    writeln!(out, "];").unwrap();
}

/// Checks the values, and returns the update key's file.
fn check(brand: &Brand, folder: &Path) -> PathBuf {
    let app = &brand.app;
    let plain = |field: &str, value: &str| {
        if value.trim().is_empty() || value.trim() != value {
            fail(&format!(
                "[app] {field} must not be empty or start or end with spaces"
            ));
        }
        // It goes into file names, the macOS Info.plist (XML), installer
        // scripts and `sed` replacements.
        if value.chars().any(char::is_control)
            || value.contains(['"', '&', '<', '>', '|', '/', '\\', ':', '*', '?', '$', '`'])
        {
            fail(&format!(
                "[app] {field} must not hold control characters or any of \" & < > | / \\ : * ? $ `"
            ));
        }
    };
    plain("name", &app.name);
    plain("vendor", &app.vendor);
    plain("keychain", &app.keychain);
    plain("data_folder", &app.data_folder);
    if app.data_folder.contains(['/', '\\', ':']) || app.data_folder.starts_with('.') {
        fail("[app] data_folder must be one folder name, without slashes, colons or a leading dot");
    }
    if !app.id.contains('.')
        || !app
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        fail("[app] id must be reverse-DNS, like com.example.app (letters, digits, dots, hyphens)");
    }
    for (field, url) in [
        ("repository", &brand.links.repository),
        ("releases", &brand.links.releases),
    ] {
        if !url.starts_with("https://") {
            fail(&format!("[links] {field} must start with https://"));
        }
    }
    let updates = &brand.updates;
    if updates.enabled && updates.sources.is_empty() {
        fail("[updates] is enabled but lists no [[updates.sources]]");
    }
    for source in &updates.sources {
        if source.name.trim().is_empty() {
            fail("every [[updates.sources]] needs a name");
        }
        if !source.manifest.starts_with("https://") || !source.files.starts_with("https://") {
            fail(&format!(
                "update source `{}`: manifest and files must start with https://",
                source.name
            ));
        }
        if !source.files.ends_with('/') {
            fail(&format!(
                "update source `{}`: files must end with /",
                source.name
            ));
        }
    }
    if updates.key.contains(['/', '\\']) {
        fail("[updates] key is a file name in brand/, without slashes");
    }
    let key = folder.join(&updates.key);
    match std::fs::read_to_string(&key) {
        Ok(text) if !text.trim().is_empty() => {}
        Ok(_) => fail(&format!("brand/{} is empty", updates.key)),
        Err(err) => fail(&format!("cannot read brand/{}: {err}", updates.key)),
    }
    key
}

fn fail(message: &str) -> ! {
    panic!("\n\nbrand/brand.toml: {message}\n(see docs/customizing.md)\n\n");
}
