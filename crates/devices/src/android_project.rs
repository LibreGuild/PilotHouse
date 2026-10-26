//! Android app modules in a project folder, and their build variants,
//! read from the Gradle build files without running Gradle (that takes
//! seconds; this takes milliseconds).
//!
//! Variants are the product flavors (one per flavor dimension, in the
//! dimensions' order) times the build types, as the Android Gradle plugin
//! names them: `freeStagingDebug` builds with `assembleFreeStagingDebug`
//! into `build/outputs/apk/freeStaging/debug/`.

use std::path::{Path, PathBuf};

/// An app module (one that applies `com.android.application`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidModule {
    /// The Gradle path: `:app`, `:apps:phone`.
    pub path: String,
    pub dir: PathBuf,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// `freeStagingDebug`.
    pub name: String,
    pub flavors: Vec<String>,
    pub build_type: String,
}

impl Variant {
    fn new(flavors: Vec<String>, build_type: String) -> Self {
        let name = lower_camel(flavors.iter().chain([&build_type]));
        Self {
            name,
            flavors,
            build_type,
        }
    }

    /// `FreeStagingDebug`, for task names.
    pub fn task_suffix(&self) -> String {
        capitalize(&self.name)
    }

    /// Where the plugin puts this variant's APKs.
    pub fn apk_dir(&self, module: &Path) -> PathBuf {
        let mut dir = module.join("build").join("outputs").join("apk");
        if !self.flavors.is_empty() {
            dir.push(lower_camel(&self.flavors));
        }
        dir.join(&self.build_type)
    }
}

/// The Gradle root of `folder`: itself when it has a wrapper.
pub fn gradle_root(folder: &Path) -> Option<PathBuf> {
    folder
        .join(if cfg!(windows) {
            "gradlew.bat"
        } else {
            "gradlew"
        })
        .is_file()
        .then(|| folder.to_path_buf())
}

/// App modules under `root`, two levels deep at most (`app`, `apps/phone`).
pub fn find_modules(root: &Path) -> Vec<AndroidModule> {
    let mut modules = Vec::new();
    visit(root, root, 0, &mut modules);
    modules.sort_by(|a, b| {
        // `:app` first, then by path.
        (a.path != ":app", &a.path).cmp(&(b.path != ":app", &b.path))
    });
    modules
}

fn visit(root: &Path, dir: &Path, depth: usize, modules: &mut Vec<AndroidModule>) {
    if depth > 0
        && let Some(text) = build_file(dir)
        && is_app_module(&text)
    {
        let relative = dir.strip_prefix(root).unwrap_or(dir);
        let path = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":");
        modules.push(AndroidModule {
            path: format!(":{path}"),
            dir: dir.to_path_buf(),
            variants: variants(&text),
        });
        return;
    }
    if depth == 2 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || matches!(name.as_ref(), "build" | "gradle" | "node_modules") {
            continue;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            visit(root, &entry.path(), depth + 1, modules);
        }
    }
}

fn build_file(dir: &Path) -> Option<String> {
    ["build.gradle.kts", "build.gradle"]
        .iter()
        .find_map(|name| std::fs::read_to_string(dir.join(name)).ok())
}

/// The application plugin, by id or through a version catalog alias.
fn is_app_module(build: &str) -> bool {
    build.contains("com.android.application")
        || build.contains("android.application")
        || build.contains("libs.plugins.android.app")
}

/// The module's variants; `debug` and `release` when nothing else is said.
pub fn variants(build: &str) -> Vec<Variant> {
    let mut build_types = vec!["debug".to_string(), "release".to_string()];
    if let Some(block) = block(build, "buildTypes") {
        for (name, _) in entries(block) {
            if !build_types.contains(&name) {
                build_types.push(name);
            }
        }
    }
    let dimensions_order = flavor_dimensions(build);
    let mut dimensions: Vec<(String, Vec<String>)> = dimensions_order
        .iter()
        .map(|dimension| (dimension.clone(), Vec::new()))
        .collect();
    if let Some(block) = block(build, "productFlavors") {
        for (name, body) in entries(block) {
            let dimension = quoted_after(body, "dimension").unwrap_or_default();
            match dimensions.iter_mut().find(|(known, _)| *known == dimension) {
                Some((_, flavors)) => flavors.push(name),
                None => dimensions.push((dimension, vec![name])),
            }
        }
    }
    let mut combos: Vec<Vec<String>> = vec![Vec::new()];
    for (_, flavors) in dimensions.iter().filter(|(_, flavors)| !flavors.is_empty()) {
        combos = combos
            .iter()
            .flat_map(|combo| {
                flavors.iter().map(move |flavor| {
                    let mut next = combo.clone();
                    next.push(flavor.clone());
                    next
                })
            })
            .collect();
    }
    combos
        .into_iter()
        .flat_map(|flavors| {
            build_types
                .iter()
                .map(move |build_type| Variant::new(flavors.clone(), build_type.clone()))
        })
        .collect()
}

/// The body of the first `name { … }` block.
fn block<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(found) = text[from..].find(name) {
        let start = from + found;
        from = start + name.len();
        // A whole word: not `myBuildTypes`.
        let before = text[..start].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
            continue;
        }
        let rest = text[from..].trim_start();
        if let Some(body) = rest.strip_prefix('{') {
            let end = matching_brace(body)?;
            return Some(&body[..end]);
        }
    }
    None
}

/// The index of the `}` closing a block whose `{` came just before `body`.
fn matching_brace(body: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (ix, c) in body.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(ix),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The named entries of a block: `release { … }`, `create("staging") { … }`,
/// `getByName("debug") { … }`.
fn entries(block: &str) -> Vec<(String, &str)> {
    let mut found = Vec::new();
    let mut rest = block;
    while let Some(open) = rest.find('{') {
        let header = rest[..open].lines().last().unwrap_or("").trim();
        let body_start = open + 1;
        let Some(end) = matching_brace(&rest[body_start..]) else {
            break;
        };
        let body = &rest[body_start..body_start + end];
        if let Some(name) = entry_name(header) {
            found.push((name, body));
        }
        rest = &rest[body_start + end + 1..];
    }
    found
}

fn entry_name(header: &str) -> Option<String> {
    if let Some(name) = first_quoted(header) {
        return Some(name);
    }
    let word = header.split_whitespace().last()?;
    let skip = ["all", "configureEach", "each", "forEach", "matching"];
    (word.chars().all(|c| c.is_alphanumeric() || c == '_') && !skip.contains(&word))
        .then(|| word.to_string())
}

/// `flavorDimensions "a", "b"`, `flavorDimensions += listOf("a", "b")`…
fn flavor_dimensions(build: &str) -> Vec<String> {
    build
        .lines()
        .filter(|line| line.trim_start().starts_with("flavorDimensions"))
        .flat_map(all_quoted)
        .collect()
}

/// The first quoted word after `key` in `text`.
fn quoted_after(text: &str, key: &str) -> Option<String> {
    let at = text.find(key)?;
    first_quoted(text[at + key.len()..].lines().next()?)
}

fn first_quoted(text: &str) -> Option<String> {
    all_quoted(text).into_iter().next()
}

fn all_quoted(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut parts = text.split(['"', '\'']);
    parts.next();
    while let Some(word) = parts.next() {
        if !word.is_empty() {
            words.push(word.to_string());
        }
        parts.next();
    }
    words
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn lower_camel<'a>(words: impl IntoIterator<Item = &'a String>) -> String {
    let mut name = String::new();
    for word in words {
        if name.is_empty() {
            name.push_str(word);
        } else {
            name.push_str(&capitalize(word));
        }
    }
    name
}

impl Variant {
    /// The application id and APK of the last build of this variant.
    ///
    /// Built the way Android Studio builds (with injected properties such
    /// as the device's ABI), the plugin leaves the APK among its
    /// intermediates and writes a redirect file naming its listing; a plain
    /// build puts it in `build/outputs/apk`.
    pub fn built_apk(&self, module: &Path) -> Option<(String, PathBuf)> {
        let redirect = module
            .join("build/intermediates/apk_ide_redirect_file")
            .join(&self.name)
            .join(format!(
                "create{}ApkListingFileRedirect",
                self.task_suffix()
            ))
            .join("redirect.txt");
        let redirected = std::fs::read_to_string(&redirect).ok().and_then(|text| {
            let listing = text
                .lines()
                .find_map(|line| line.strip_prefix("listingFile="))?
                .trim();
            read_listing(&redirect.parent()?.join(listing))
        });
        let plain = read_listing(&self.apk_dir(module).join("output-metadata.json"));
        let intermediate = read_listing(
            &module
                .join("build/intermediates/apk")
                .join(&self.name)
                .join("output-metadata.json"),
        );
        // The newest of them: a redirect left by an earlier Android Studio
        // build must not win over the APK this build just made.
        [redirected, plain, intermediate]
            .into_iter()
            .flatten()
            .max_by_key(|(_, apk)| {
                std::fs::metadata(apk)
                    .and_then(|meta| meta.modified())
                    .unwrap_or(std::time::UNIX_EPOCH)
            })
    }
}

/// An APK listing (`output-metadata.json`): the application id and the
/// APK beside it.
fn read_listing(listing: &Path) -> Option<(String, PathBuf)> {
    let text = std::fs::read_to_string(listing).ok()?;
    let metadata: serde_json::Value = serde_json::from_str(&text).ok()?;
    let id = metadata["applicationId"].as_str()?.to_string();
    let file = metadata["elements"]
        .as_array()?
        .iter()
        .find_map(|element| element["outputFile"].as_str())?;
    let apk = listing.parent()?.join(file);
    apk.is_file().then_some((id, apk))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(build: &str) -> Vec<String> {
        variants(build)
            .into_iter()
            .map(|variant| variant.name)
            .collect()
    }

    #[test]
    fn plain_modules_have_debug_and_release() {
        assert_eq!(names("android { }"), ["debug", "release"]);
    }

    #[test]
    fn reads_kotlin_build_types_and_flavors() {
        let build = r#"
android {
    flavorDimensions += listOf("tier", "env")
    buildTypes {
        getByName("release") { isMinifyEnabled = true }
        create("benchmark") { initWith(getByName("release")) }
    }
    productFlavors {
        create("staging") { dimension = "env" }
        create("free") { dimension = "tier" }
        create("paid") { dimension = "tier" }
    }
}"#;
        let all = names(build);
        assert_eq!(all.len(), 2 * 3);
        assert_eq!(
            &all[..3],
            [
                "freeStagingDebug",
                "freeStagingRelease",
                "freeStagingBenchmark"
            ]
        );
        let variant = &variants(build)[0];
        assert_eq!(variant.task_suffix(), "FreeStagingDebug");
        assert_eq!(
            variant.apk_dir(Path::new("app")),
            Path::new("app/build/outputs/apk/freeStaging/debug")
        );
    }

    #[test]
    fn reads_groovy_flavors() {
        let build = "android {\n  flavorDimensions 'version'\n  buildTypes {\n    release {\n      minifyEnabled true\n    }\n    qa { }\n  }\n  productFlavors {\n    demo {\n      dimension 'version'\n    }\n    full {\n      dimension 'version'\n    }\n  }\n}";
        assert_eq!(
            names(build),
            [
                "demoDebug",
                "demoRelease",
                "demoQa",
                "fullDebug",
                "fullRelease",
                "fullQa"
            ]
        );
    }

    #[test]
    fn follows_the_ide_redirect_to_the_apk() {
        let module = std::env::temp_dir().join(format!("ph-android-apk-{}", std::process::id()));
        let variant = &variants("")[0];
        let redirect = module.join(
            "build/intermediates/apk_ide_redirect_file/debug/createDebugApkListingFileRedirect",
        );
        let apk_dir = module.join("build/intermediates/apk/debug");
        std::fs::create_dir_all(&redirect).unwrap();
        std::fs::create_dir_all(&apk_dir).unwrap();
        std::fs::write(
            redirect.join("redirect.txt"),
            "#- File Locator -\nlistingFile=../../../apk/debug/output-metadata.json\n",
        )
        .unwrap();
        std::fs::write(
            apk_dir.join("output-metadata.json"),
            r#"{"applicationId":"com.example.app","elements":[{"outputFile":"app-debug.apk"}]}"#,
        )
        .unwrap();
        assert!(variant.built_apk(&module).is_none());
        std::fs::write(apk_dir.join("app-debug.apk"), "apk").unwrap();
        let (id, apk) = variant.built_apk(&module).unwrap();
        assert_eq!(id, "com.example.app");
        assert!(apk.ends_with("app-debug.apk"));
        let _ = std::fs::remove_dir_all(&module);
    }

    #[test]
    fn finds_app_modules() {
        let root = std::env::temp_dir().join(format!("ph-android-{}", std::process::id()));
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::create_dir_all(root.join("apps/wear")).unwrap();
        std::fs::write(
            root.join("app/build.gradle.kts"),
            "plugins { alias(libs.plugins.android.application) }",
        )
        .unwrap();
        std::fs::write(
            root.join("lib/build.gradle.kts"),
            "plugins { id(\"com.android.library\") }",
        )
        .unwrap();
        std::fs::write(
            root.join("apps/wear/build.gradle"),
            "apply plugin: 'com.android.application'",
        )
        .unwrap();
        let paths: Vec<String> = find_modules(&root)
            .into_iter()
            .map(|module| module.path)
            .collect();
        assert_eq!(paths, [":app", ":apps:wear"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
