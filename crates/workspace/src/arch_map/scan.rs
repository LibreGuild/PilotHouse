//! Finding a project's parts and how they depend on each other, from its
//! manifests: the members of a Cargo or npm workspace and their internal
//! dependencies, or else its top-level folders. Read locally, nothing run.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// One part of the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    /// Relative to the project root, with `/`.
    pub folder: String,
    pub files: usize,
    pub lines: usize,
    /// Its largest languages by lines, e.g. `["Rust", "TOML"]`.
    pub languages: Vec<&'static str>,
    /// The parts it depends on (indexes).
    pub depends_on: Vec<usize>,
    /// The dependencies worth a line: those not already reached through
    /// another one (so a part using `core` and `ui`, where `ui` uses
    /// `core`, gets one line, to `ui`).
    pub drawn: Vec<usize>,
    /// 0 for parts depending on nothing inside the project, then one more
    /// than the highest part it depends on.
    pub layer: usize,
}

/// What kind of manifest the parts came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Cargo,
    Npm,
    Folders,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Map {
    pub source: Source,
    pub parts: Vec<Part>,
}

/// Most files counted per part, so a huge folder cannot stall the scan.
const MAX_FILES: usize = 20_000;

pub fn scan(root: &Path) -> Map {
    let (source, members) = cargo_members(root)
        .map(|members| (Source::Cargo, members))
        .or_else(|| npm_members(root).map(|members| (Source::Npm, members)))
        .unwrap_or_else(|| (Source::Folders, folders(root)));
    let index: HashMap<String, usize> = members
        .iter()
        .enumerate()
        .map(|(ix, (name, _, _))| (name.clone(), ix))
        .collect();
    let mut parts: Vec<Part> = members
        .iter()
        .map(|(name, folder, deps)| {
            let (files, lines, languages) = measure(&root.join(folder));
            let mut depends_on: Vec<usize> = deps
                .iter()
                .filter_map(|dep| index.get(dep).copied())
                .filter(|dep| index.get(name) != Some(dep))
                .collect();
            depends_on.sort_unstable();
            depends_on.dedup();
            Part {
                name: name.clone(),
                folder: folder.clone(),
                files,
                lines,
                languages,
                drawn: Vec::new(),
                depends_on,
                layer: 0,
            }
        })
        .collect();
    layer(&mut parts);
    reduce(&mut parts);
    Map { source, parts }
}

/// Fills each part's `drawn`: the transitive reduction of `depends_on`.
pub fn reduce(parts: &mut [Part]) {
    let reaches = |from: usize, to: usize, parts: &[Part]| {
        let mut stack = vec![from];
        let mut seen = vec![false; parts.len()];
        while let Some(at) = stack.pop() {
            if at == to {
                return true;
            }
            if std::mem::replace(&mut seen[at], true) {
                continue;
            }
            stack.extend(parts[at].depends_on.iter().copied());
        }
        false
    };
    let drawn: Vec<Vec<usize>> = parts
        .iter()
        .map(|part| {
            part.depends_on
                .iter()
                .copied()
                .filter(|&dep| {
                    !part
                        .depends_on
                        .iter()
                        .any(|&other| other != dep && reaches(other, dep, parts))
                })
                .collect()
        })
        .collect();
    for (part, drawn) in parts.iter_mut().zip(drawn) {
        part.drawn = drawn;
    }
}

/// Gives each part its layer (cycles are cut, not followed).
pub fn layer(parts: &mut [Part]) {
    fn depth(
        ix: usize,
        parts: &[Part],
        memo: &mut [Option<usize>],
        visiting: &mut [bool],
    ) -> usize {
        if let Some(depth) = memo[ix] {
            return depth;
        }
        if visiting[ix] {
            return 0;
        }
        visiting[ix] = true;
        let depth = parts[ix]
            .depends_on
            .iter()
            .map(|&dep| depth(dep, parts, memo, visiting) + 1)
            .max()
            .unwrap_or(0);
        visiting[ix] = false;
        memo[ix] = Some(depth);
        depth
    }
    let mut memo = vec![None; parts.len()];
    let mut visiting = vec![false; parts.len()];
    let layers: Vec<usize> = (0..parts.len())
        .map(|ix| depth(ix, parts, &mut memo, &mut visiting))
        .collect();
    for (part, layer) in parts.iter_mut().zip(layers) {
        part.layer = layer;
    }
}

/// `(name, folder, dependency names)` of each Cargo workspace member.
fn cargo_members(root: &Path) -> Option<Vec<(String, String, Vec<String>)>> {
    let manifest: toml::Table = std::fs::read_to_string(root.join("Cargo.toml"))
        .ok()?
        .parse()
        .ok()?;
    let workspace = manifest.get("workspace")?.as_table()?;
    // A dependency on another member is listed under its package name,
    // whether as `{ path = … }` or `.workspace = true`.
    let mut folders = Vec::new();
    for pattern in workspace
        .get("members")?
        .as_array()?
        .iter()
        .filter_map(|m| m.as_str())
    {
        folders.extend(expand(root, pattern));
    }
    let members = folders
        .into_iter()
        .filter_map(|folder| {
            let member: toml::Table =
                std::fs::read_to_string(root.join(&folder).join("Cargo.toml"))
                    .ok()?
                    .parse()
                    .ok()?;
            let name = member.get("package")?.get("name")?.as_str()?.to_string();
            let deps = ["dependencies", "dev-dependencies", "build-dependencies"]
                .iter()
                .filter_map(|section| member.get(*section)?.as_table())
                .flat_map(|table| table.keys().cloned())
                .collect();
            Some((name, folder, deps))
        })
        .collect::<Vec<_>>();
    (!members.is_empty()).then_some(members)
}

/// `(name, folder, dependency names)` of each npm/yarn/pnpm workspace package.
fn npm_members(root: &Path) -> Option<Vec<(String, String, Vec<String>)>> {
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("package.json")).ok()?).ok()?;
    let patterns: Vec<String> = match manifest.get("workspaces")? {
        serde_json::Value::Array(list) => list
            .iter()
            .filter_map(|p| p.as_str().map(str::to_string))
            .collect(),
        other => other
            .get("packages")?
            .as_array()?
            .iter()
            .filter_map(|p| p.as_str().map(str::to_string))
            .collect(),
    };
    let members = patterns
        .iter()
        .flat_map(|pattern| expand(root, pattern))
        .filter_map(|folder| {
            let package: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(root.join(&folder).join("package.json")).ok()?,
            )
            .ok()?;
            let name = package.get("name")?.as_str()?.to_string();
            let deps = ["dependencies", "devDependencies", "peerDependencies"]
                .iter()
                .filter_map(|section| package.get(*section)?.as_object())
                .flat_map(|table| table.keys().cloned())
                .collect();
            Some((name, folder, deps))
        })
        .collect::<Vec<_>>();
    (!members.is_empty()).then_some(members)
}

/// Top-level folders, for projects without a workspace manifest.
fn folders(root: &Path) -> Vec<(String, String, Vec<String>)> {
    const SKIP: [&str; 8] = [
        "node_modules",
        "target",
        "build",
        "dist",
        "out",
        "vendor",
        "__pycache__",
        "venv",
    ];
    let mut found: Vec<_> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (!name.starts_with('.') && !SKIP.contains(&name.as_str()))
                .then(|| (name.clone(), name, Vec::new()))
        })
        .collect();
    found.sort();
    found
}

/// The folders a workspace pattern names: `crates/*`, `packages/**`, or a
/// plain folder.
fn expand(root: &Path, pattern: &str) -> Vec<String> {
    let pattern = pattern.trim_end_matches('/');
    match pattern
        .strip_suffix("/*")
        .or_else(|| pattern.strip_suffix("/**"))
    {
        Some(parent) => {
            let mut folders: Vec<String> = std::fs::read_dir(root.join(parent))
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .map(|entry| format!("{parent}/{}", entry.file_name().to_string_lossy()))
                .collect();
            folders.sort();
            folders
        }
        None if root.join(pattern).is_dir() => vec![pattern.to_string()],
        None => Vec::new(),
    }
}

/// Files, lines and the main languages under `folder` (ignored files left
/// out).
fn measure(folder: &Path) -> (usize, usize, Vec<&'static str>) {
    let mut files = 0;
    let mut lines = 0;
    let mut by_language: BTreeMap<&'static str, usize> = BTreeMap::new();
    for entry in ignore::WalkBuilder::new(folder).build().flatten() {
        if files >= MAX_FILES {
            break;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(language) = language(path) else {
            continue;
        };
        files += 1;
        let count = std::fs::read(path)
            .map(|bytes| bytes.iter().filter(|&&b| b == b'\n').count())
            .unwrap_or_default();
        lines += count;
        *by_language.entry(language).or_default() += count;
    }
    let mut languages: Vec<_> = by_language.into_iter().collect();
    languages.sort_by_key(|(_, lines)| std::cmp::Reverse(*lines));
    (
        files,
        lines,
        languages
            .into_iter()
            .take(2)
            .map(|(language, _)| language)
            .collect(),
    )
}

/// The language of a source file, by extension; `None` for other files
/// (manifests and docs are not counted as code).
fn language(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "rs" => "Rust",
        "ts" | "tsx" | "mts" | "cts" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "py" => "Python",
        "go" => "Go",
        "kt" | "kts" => "Kotlin",
        "java" => "Java",
        "swift" => "Swift",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" => "C++",
        "cs" => "C#",
        "rb" => "Ruby",
        "dart" => "Dart",
        "php" => "PHP",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "css" | "scss" => "CSS",
        "html" => "HTML",
        "sql" => "SQL",
        _ => return None,
    })
}

/// The map as text, to give an agent: each part, its size and what it uses.
pub fn describe(map: &Map) -> String {
    let mut text =
        String::from("Parts of this project (from its manifests), each with what it depends on:\n");
    for part in &map.parts {
        let uses: Vec<&str> = part
            .depends_on
            .iter()
            .map(|&dep| map.parts[dep].name.as_str())
            .collect();
        text.push_str(&format!(
            "- {} ({}): {} files, {} lines, {}{}\n",
            part.name,
            part.folder,
            part.files,
            part.lines,
            if part.languages.is_empty() {
                "no source".to_string()
            } else {
                part.languages.join(", ")
            },
            if uses.is_empty() {
                String::new()
            } else {
                format!("; uses {}", uses.join(", "))
            }
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn cargo_workspaces_become_layered_parts() {
        let root = std::env::temp_dir().join(format!("ph-arch-cargo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n[workspace.dependencies]\ncore = { path = \"crates/core\" }\n",
        );
        write(
            &root.join("crates/core/Cargo.toml"),
            "[package]\nname = \"core\"\n[dependencies]\nserde = \"1\"\n",
        );
        write(
            &root.join("crates/core/src/lib.rs"),
            "pub fn a() {}\npub fn b() {}\n",
        );
        write(
            &root.join("crates/ui/Cargo.toml"),
            "[package]\nname = \"ui\"\n[dependencies]\ncore.workspace = true\n",
        );
        write(
            &root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\n[dependencies]\nui = { path = \"../ui\" }\ncore.workspace = true\n",
        );

        let map = scan(&root);
        assert_eq!(map.source, Source::Cargo);
        let by_name: HashMap<_, _> = map
            .parts
            .iter()
            .map(|part| (part.name.as_str(), part))
            .collect();
        assert_eq!(by_name["core"].layer, 0);
        assert_eq!(by_name["ui"].layer, 1);
        assert_eq!(by_name["app"].layer, 2);
        let ui = map.parts.iter().position(|part| part.name == "ui").unwrap();
        assert_eq!(
            by_name["app"].drawn,
            [ui],
            "app → core is implied by app → ui → core"
        );
        assert_eq!((by_name["core"].files, by_name["core"].lines), (1, 2));
        assert_eq!(by_name["core"].languages, ["Rust"]);
        assert!(
            describe(&map)
                .contains("- app (crates/app): 0 files, 0 lines, no source; uses core, ui")
        );
    }

    #[test]
    fn npm_workspaces_and_plain_folders() {
        let root = std::env::temp_dir().join(format!("ph-arch-npm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(
            &root.join("package.json"),
            r#"{"workspaces": {"packages": ["packages/*"]}}"#,
        );
        write(
            &root.join("packages/api/package.json"),
            r#"{"name": "@acme/api"}"#,
        );
        write(
            &root.join("packages/web/package.json"),
            r#"{"name": "@acme/web", "dependencies": {"@acme/api": "*", "react": "18"}}"#,
        );
        write(
            &root.join("packages/web/src/App.tsx"),
            "export const App = 1;\n",
        );
        let map = scan(&root);
        assert_eq!(map.source, Source::Npm);
        assert_eq!(map.parts[1].depends_on, [0]);
        assert_eq!(map.parts[1].languages, ["TypeScript"]);

        let plain = std::env::temp_dir().join(format!("ph-arch-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&plain);
        write(&plain.join("src/main.py"), "print(1)\n");
        write(&plain.join("node_modules/x/index.js"), "");
        write(&plain.join(".git/HEAD"), "");
        let map = scan(&plain);
        assert_eq!(map.source, Source::Folders);
        assert_eq!(
            map.parts
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["src"]
        );
    }

    #[test]
    fn cycles_do_not_loop() {
        let part = |deps: Vec<usize>| Part {
            name: String::new(),
            folder: String::new(),
            files: 0,
            lines: 0,
            languages: Vec::new(),
            depends_on: deps,
            drawn: Vec::new(),
            layer: 0,
        };
        let mut parts = vec![part(vec![1]), part(vec![0])];
        layer(&mut parts);
        reduce(&mut parts);
        assert!(parts.iter().all(|part| part.layer <= 2));
    }
}
