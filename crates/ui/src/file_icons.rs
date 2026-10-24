//! Icons for files and folders by type, so a list of files reads at a
//! glance: [`file_icon_element`] and [`folder_icon_element`] draw a full
//! colour icon for each known kind (Python's snake, JSON's braces, …) and a
//! tinted outline icon for the rest; [`file_icon`] is the outline icon
//! alone, for places drawn in one colour.
//!
//! The colour icons are from the Material Icon Theme (MIT, see
//! `assets/file-icons/LICENSE`), bundled so nothing is fetched. Each SVG is
//! drawn at 64 px and scaled down, so it stays sharp at any size.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::{
    AnyElement, Hsla, Image, ImageFormat, IntoElement as _, Pixels, Styled as _, img, rgb,
};

use crate::Icons;

/// The icon and tint for a file named `name` (a bare name or a path).
pub fn file_icon(name: &str) -> (Icons, Hsla) {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = file.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    let (icon, color) = match (lower.as_str(), extension) {
        (
            "cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" | "bun.lockb"
            | "poetry.lock" | "gemfile.lock",
            _,
        ) => (Icons::FileLock, 0x8b93a1),
        (
            "dockerfile"
            | "containerfile"
            | "compose.yaml"
            | "compose.yml"
            | "docker-compose.yml"
            | "docker-compose.yaml",
            _,
        ) => (Icons::Container, 0x2496ed),
        ("makefile" | "justfile" | "gradlew" | "gradlew.bat", _) => (Icons::FileTerminal, 0x6e9f52),
        ("license" | "license.md" | "license.txt" | "notice", _) => (Icons::FileText, 0xc9a227),
        (_, "rs") => (Icons::FileCode, 0xde7c4f),
        (_, "ts" | "tsx" | "mts" | "cts") => (Icons::FileCode, 0x3178c6),
        (_, "js" | "jsx" | "mjs" | "cjs") => (Icons::FileCode, 0xe8c547),
        (_, "py" | "pyi") => (Icons::FileCode, 0x4b8bbe),
        (_, "go") => (Icons::FileCode, 0x00add8),
        (_, "kt" | "kts") => (Icons::FileCode, 0xa97bff),
        (_, "java") => (Icons::FileCode, 0xe76f00),
        (_, "swift") => (Icons::FileCode, 0xf05138),
        (_, "c" | "h") => (Icons::FileCode, 0x5c8dbc),
        (_, "cpp" | "cc" | "cxx" | "hpp" | "hh") => (Icons::FileCode, 0x659ad2),
        (_, "cs") => (Icons::FileCode, 0x68217a),
        (_, "rb") => (Icons::FileCode, 0xcc342d),
        (_, "php") => (Icons::FileCode, 0x777bb4),
        (_, "dart") => (Icons::FileCode, 0x00b4ab),
        (_, "lua") => (Icons::FileCode, 0x4c6fd1),
        (_, "zig") => (Icons::FileCode, 0xf7a41d),
        (_, "vue") => (Icons::FileCode, 0x42b883),
        (_, "svelte") => (Icons::FileCode, 0xff3e00),
        (_, "html" | "htm") => (Icons::FileCode, 0xe44d26),
        (_, "css" | "scss" | "sass" | "less") => (Icons::FileCode, 0x2965f1),
        (_, "json" | "jsonc" | "json5") => (Icons::Braces, 0xcbcb41),
        (_, "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "properties" | "env") => {
            (Icons::FileCog, 0x8b93a1)
        }
        (_, "xml" | "plist" | "gradle") => (Icons::FileCog, 0xe37933),
        (_, "md" | "mdx" | "markdown" | "rst" | "adoc") => (Icons::FileText, 0x519aba),
        (_, "txt" | "log") => (Icons::FileText, 0x8b93a1),
        (_, "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd") => {
            (Icons::FileTerminal, 0x6e9f52)
        }
        (_, "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" | "avif") => {
            (Icons::FileImage, 0xa074c4)
        }
        (_, "zip" | "tar" | "gz" | "tgz" | "xz" | "7z" | "rar" | "jar" | "apk" | "aab") => {
            (Icons::FileArchive, 0xb08850)
        }
        (_, "csv" | "tsv" | "xlsx" | "xls") => (Icons::FileSpreadsheet, 0x3b9a57),
        (_, "sql" | "db" | "sqlite" | "sqlite3") => (Icons::Database, 0xdad8d8),
        (_, "lock") => (Icons::FileLock, 0x8b93a1),
        _ => (Icons::File, 0x8b93a1),
    };
    (icon, rgb(color).into())
}

/// Bundles the colour icons by name.
macro_rules! svgs {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/file-icons/", $name, ".svg")) as &[u8])),*]
    };
}

static SVGS: &[(&str, &[u8])] = svgs![
    "android",
    "c",
    "certificate",
    "console",
    "cpp",
    "csharp",
    "css",
    "dart",
    "database",
    "docker",
    "document",
    "elixir",
    "folder",
    "folder-components",
    "folder-config",
    "folder-dist",
    "folder-docs",
    "folder-git",
    "folder-github",
    "folder-images",
    "folder-lib",
    "folder-log",
    "folder-node",
    "folder-public",
    "folder-scripts",
    "folder-src",
    "folder-target",
    "folder-test",
    "folder-vscode",
    "git",
    "gitlab",
    "go",
    "gradle",
    "graphql",
    "h",
    "haskell",
    "hpp",
    "html",
    "image",
    "java",
    "javascript",
    "json",
    "kotlin",
    "less",
    "license",
    "lock",
    "log",
    "lua",
    "makefile",
    "markdown",
    "next",
    "nix",
    "nodejs",
    "npm",
    "pdf",
    "php",
    "pnpm",
    "powershell",
    "proto",
    "python",
    "react",
    "react_ts",
    "readme",
    "ruby",
    "rust",
    "sass",
    "scala",
    "settings",
    "svelte",
    "svg",
    "swift",
    "table",
    "terraform",
    "toml",
    "tsconfig",
    "typescript",
    "vite",
    "vue",
    "xml",
    "yaml",
    "yarn",
    "zig",
    "zip",
];

/// The decoded icons, made once (the image cache keys on them).
static IMAGES: LazyLock<HashMap<&'static str, Arc<Image>>> = LazyLock::new(|| {
    SVGS.iter()
        .map(|(name, bytes)| {
            (
                *name,
                Arc::new(Image::from_bytes(ImageFormat::Svg, bytes.to_vec())),
            )
        })
        .collect()
});

/// The colour icon for a file named `name` (a bare name or a path), if its
/// kind has one.
pub fn file_svg_name(name: &str) -> Option<&'static str> {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let lower = file.to_ascii_lowercase();
    let extension = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    let by_name = match lower.as_str() {
        "dockerfile"
        | "containerfile"
        | "compose.yaml"
        | "compose.yml"
        | "docker-compose.yml"
        | "docker-compose.yaml"
        | ".dockerignore" => Some("docker"),
        "makefile" | "justfile" | "gnumakefile" => Some("makefile"),
        "package.json" | "package-lock.json" | ".npmrc" => Some("npm"),
        "yarn.lock" | ".yarnrc" | ".yarnrc.yml" => Some("yarn"),
        "pnpm-lock.yaml" | "pnpm-workspace.yaml" => Some("pnpm"),
        "cargo.toml" | "cargo.lock" | "rust-toolchain" | "rust-toolchain.toml" => Some("rust"),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitkeep" => Some("git"),
        ".gitlab-ci.yml" => Some("gitlab"),
        "gradlew" | "gradlew.bat" | "gradle.properties" => Some("gradle"),
        "androidmanifest.xml" => Some("android"),
        ".nvmrc" | ".node-version" => Some("nodejs"),
        _ if lower.starts_with("license") || lower.starts_with("copying") || lower == "notice" => {
            Some("license")
        }
        _ if lower.starts_with("readme") => Some("readme"),
        _ if lower.starts_with("tsconfig") && extension == "json" => Some("tsconfig"),
        _ if lower.starts_with("vite.config.") => Some("vite"),
        _ if lower.starts_with("next.config.") => Some("next"),
        _ if lower.starts_with(".env") => Some("settings"),
        _ => None,
    };
    by_name.or(match extension {
        "py" | "pyi" | "pyw" | "ipynb" => Some("python"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "ts" | "mts" | "cts" => Some("typescript"),
        "jsx" => Some("react"),
        "tsx" => Some("react_ts"),
        "rs" => Some("rust"),
        "go" => Some("go"),
        "java" => Some("java"),
        "kt" | "kts" => Some("kotlin"),
        "swift" => Some("swift"),
        "c" | "m" => Some("c"),
        "h" => Some("h"),
        "cpp" | "cc" | "cxx" | "mm" => Some("cpp"),
        "hpp" | "hh" | "hxx" => Some("hpp"),
        "cs" => Some("csharp"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "dart" => Some("dart"),
        "lua" => Some("lua"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "scss" | "sass" => Some("sass"),
        "less" => Some("less"),
        "json" | "jsonc" | "json5" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "xml" | "plist" | "xsd" => Some("xml"),
        "md" | "mdx" | "markdown" => Some("markdown"),
        "txt" | "rst" | "adoc" | "rtf" => Some("document"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "ico" | "bmp" | "avif" | "tiff" => Some("image"),
        "svg" => Some("svg"),
        "pdf" => Some("pdf"),
        "zip" | "tar" | "gz" | "tgz" | "xz" | "7z" | "rar" | "jar" | "bz2" => Some("zip"),
        "lock" => Some("lock"),
        "sh" | "bash" | "zsh" | "fish" | "bat" | "cmd" => Some("console"),
        "ps1" | "psm1" => Some("powershell"),
        "sql" | "db" | "sqlite" | "sqlite3" => Some("database"),
        "csv" | "tsv" | "xlsx" | "xls" => Some("table"),
        "vue" => Some("vue"),
        "svelte" => Some("svelte"),
        "ini" | "cfg" | "conf" | "properties" | "env" | "editorconfig" => Some("settings"),
        "log" => Some("log"),
        "gradle" => Some("gradle"),
        "hs" => Some("haskell"),
        "ex" | "exs" => Some("elixir"),
        "scala" | "sc" => Some("scala"),
        "zig" => Some("zig"),
        "nix" => Some("nix"),
        "proto" => Some("proto"),
        "graphql" | "gql" => Some("graphql"),
        "tf" | "tfvars" => Some("terraform"),
        "pem" | "crt" | "cer" | "key" => Some("certificate"),
        _ => None,
    })
}

/// The colour icon for a folder named `name`: a few well-known ones have
/// their own, the rest the plain folder.
pub fn folder_svg_name(name: &str) -> &'static str {
    let folder = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match folder.to_ascii_lowercase().as_str() {
        "src" | "source" | "sources" => "folder-src",
        "test" | "tests" | "__tests__" | "spec" | "specs" | "e2e" => "folder-test",
        "doc" | "docs" | "documentation" => "folder-docs",
        ".git" => "folder-git",
        ".github" => "folder-github",
        "node_modules" => "folder-node",
        "config" | "configs" | ".config" | "settings" => "folder-config",
        "images" | "img" | "icons" | "assets" | "media" => "folder-images",
        "scripts" | "bin" | "tools" => "folder-scripts",
        "dist" | "build" | "out" | "release" => "folder-dist",
        "public" | "static" | "www" => "folder-public",
        "components" | "widgets" => "folder-components",
        "lib" | "libs" | "crates" | "packages" | "vendor" => "folder-lib",
        "log" | "logs" => "folder-log",
        ".vscode" => "folder-vscode",
        "target" => "folder-target",
        _ => "folder",
    }
}

/// A file's icon `size` across: its kind's colour icon, else the outline one.
pub fn file_icon_element(name: &str, size: Pixels) -> AnyElement {
    match file_svg_name(name).and_then(|svg| IMAGES.get(svg)) {
        Some(image) => img(image.clone()).flex_none().size(size).into_any_element(),
        None => {
            let (icon, tint) = file_icon(name);
            Icon::new(icon)
                .with_size(size)
                .text_color(tint)
                .into_any_element()
        }
    }
}

/// A folder's icon `size` across.
pub fn folder_icon_element(name: &str, size: Pixels) -> AnyElement {
    let image = IMAGES
        .get(folder_svg_name(name))
        .cloned()
        .expect("the plain folder icon is bundled");
    img(image).flex_none().size(size).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_icons_by_name_and_extension() {
        assert_eq!(file_icon("src/main.rs").0, Icons::FileCode);
        assert_eq!(file_icon("Cargo.lock").0, Icons::FileLock);
        assert_eq!(file_icon(r"web\package.json").0, Icons::Braces);
        assert_eq!(file_icon("Dockerfile").0, Icons::Container);
        assert_eq!(file_icon("notes").0, Icons::File);
    }

    #[test]
    fn colour_icons_by_kind() {
        assert_eq!(file_svg_name("app/main.py"), Some("python"));
        assert_eq!(file_svg_name("index.HTML"), Some("html"));
        assert_eq!(file_svg_name("tsconfig.build.json"), Some("tsconfig"));
        assert_eq!(file_svg_name("data.json"), Some("json"));
        assert_eq!(file_svg_name("README.md"), Some("readme"));
        assert_eq!(file_svg_name(".env.local"), Some("settings"));
        assert_eq!(file_svg_name("notes"), None, "no kind, no colour icon");
        assert_eq!(folder_svg_name("src"), "folder-src");
        assert_eq!(folder_svg_name("anything"), "folder");
        // Every name the mapping can return is bundled.
        for name in SVGS.iter().map(|(name, _)| *name) {
            assert!(IMAGES.contains_key(name));
        }
        for file in [
            "a.py",
            "b.rs",
            "c.tsx",
            "Dockerfile",
            "x.pem",
            "y.tf",
            "z.lock",
        ] {
            let svg = file_svg_name(file).unwrap();
            assert!(IMAGES.contains_key(svg), "{svg} is bundled");
        }
    }
}
