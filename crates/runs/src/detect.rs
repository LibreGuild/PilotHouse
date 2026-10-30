//! Run configurations found in the project folder.
//!
//! Only the top level is read, and nothing is executed: detection parses
//! `package.json`, `Makefile` and friends.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfig {
    pub name: String,
    pub command: String,
    /// Added by the user (editable), rather than detected.
    #[serde(default)]
    pub custom: bool,
    /// The folder it runs in, in a workspace with several; `None` for the
    /// primary one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<std::path::PathBuf>,
}

impl RunConfig {
    fn detected(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            custom: false,
            cwd: None,
        }
    }
}

/// Most `Makefile`/`justfile` targets worth listing.
const MAX_TARGETS: usize = 12;

pub fn detect(root: &Path) -> Vec<RunConfig> {
    let mut configs = Vec::new();
    let has = |name: &str| root.join(name).exists();

    if let Ok(text) = std::fs::read_to_string(root.join("package.json")) {
        let runner = if has("pnpm-lock.yaml") {
            "pnpm"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun run"
        } else {
            "npm run"
        };
        if let Ok(package) = serde_json::from_str::<serde_json::Value>(&text)
            && let Some(scripts) = package["scripts"].as_object()
        {
            // Common entry points first, the rest in file order.
            let mut names: Vec<&String> = scripts.keys().collect();
            names.sort_by_key(|name| {
                ["dev", "start", "build", "test", "lint"]
                    .iter()
                    .position(|first| first == name)
                    .unwrap_or(99)
            });
            // Script names come from the repository and are typed into a
            // shell (POSIX on a server, PowerShell or cmd on Windows), so only
            // names no shell reads anything into are offered.
            for name in names.into_iter().filter(|name| plain_word(name)) {
                configs.push(RunConfig::detected(
                    name.clone(),
                    format!("{runner} {name}"),
                ));
            }
        }
    }
    if has("Cargo.toml") {
        configs.push(RunConfig::detected("cargo run", "cargo run"));
        configs.push(RunConfig::detected("cargo test", "cargo test"));
    }
    if has("go.mod") {
        configs.push(RunConfig::detected("go run", "go run ."));
        configs.push(RunConfig::detected("go test", "go test ./..."));
    }
    if has("manage.py") {
        configs.push(RunConfig::detected(
            "runserver",
            "python manage.py runserver",
        ));
    }
    for entry in ["main.py", "app.py"] {
        if has(entry) && !has("manage.py") {
            configs.push(RunConfig::detected(
                format!("python {entry}"),
                format!("python {entry}"),
            ));
        }
    }
    if has("pyproject.toml") || has("pytest.ini") || has("setup.py") {
        configs.push(RunConfig::detected("pytest", "python -m pytest"));
    }
    if has("pom.xml") {
        let mvn = if has("mvnw") {
            if cfg!(windows) { "mvnw.cmd" } else { "./mvnw" }
        } else {
            "mvn"
        };
        let pom = std::fs::read_to_string(root.join("pom.xml")).unwrap_or_default();
        if pom.contains("spring-boot") {
            configs.push(RunConfig::detected(
                "spring-boot:run",
                format!("{mvn} spring-boot:run"),
            ));
        }
        configs.push(RunConfig::detected("mvn package", format!("{mvn} package")));
        configs.push(RunConfig::detected("mvn test", format!("{mvn} test")));
    }
    let dotnet = dir_has_extension(root, &["sln", "slnx", "csproj", "fsproj"]);
    if let Some(found) = dotnet {
        if found != "sln" && found != "slnx" {
            configs.push(RunConfig::detected("dotnet run", "dotnet run"));
        }
        configs.push(RunConfig::detected("dotnet build", "dotnet build"));
        configs.push(RunConfig::detected("dotnet test", "dotnet test"));
    }
    if has("CMakeLists.txt") {
        configs.push(RunConfig::detected(
            "cmake configure",
            "cmake -S . -B build",
        ));
        configs.push(RunConfig::detected("cmake build", "cmake --build build"));
    }
    if let Ok(pubspec) = std::fs::read_to_string(root.join("pubspec.yaml")) {
        let tool = if pubspec.contains("flutter:") {
            "flutter"
        } else {
            "dart"
        };
        configs.push(RunConfig::detected(
            format!("{tool} run"),
            format!("{tool} run"),
        ));
        configs.push(RunConfig::detected(
            format!("{tool} test"),
            format!("{tool} test"),
        ));
    }
    for deno in ["deno.json", "deno.jsonc"] {
        let Ok(text) = std::fs::read_to_string(root.join(deno)) else {
            continue;
        };
        // Comments in deno.jsonc: whole-line ones only.
        let json: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json)
            && let Some(tasks) = value["tasks"].as_object()
        {
            for name in tasks.keys().filter(|name| plain_word(name)) {
                configs.push(RunConfig::detected(
                    format!("deno {name}"),
                    format!("deno task {name}"),
                ));
            }
        }
        break;
    }
    if has("gradlew") {
        let gradlew = if cfg!(windows) {
            "gradlew.bat"
        } else {
            "./gradlew"
        };
        if has("app") {
            configs.push(RunConfig::detected(
                "Install debug app",
                format!("{gradlew} installDebug"),
            ));
            configs.push(RunConfig::detected(
                "Build debug APK",
                format!("{gradlew} assembleDebug"),
            ));
        } else if ["build.gradle", "build.gradle.kts"].iter().any(|file| {
            std::fs::read_to_string(root.join(file)).is_ok_and(|text| text.contains("application"))
        }) {
            configs.push(RunConfig::detected("gradle run", format!("{gradlew} run")));
        }
        configs.push(RunConfig::detected(
            "gradle build",
            format!("{gradlew} build"),
        ));
        configs.push(RunConfig::detected(
            "gradle test",
            format!("{gradlew} test"),
        ));
    }
    for compose in [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yml",
        "docker-compose.yaml",
    ] {
        if has(compose) {
            configs.push(RunConfig::detected("compose up", "docker compose up"));
            break;
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("Makefile")) {
        for target in make_targets(&text).into_iter().take(MAX_TARGETS) {
            configs.push(RunConfig::detected(
                format!("make {target}"),
                format!("make {target}"),
            ));
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("justfile")) {
        for recipe in make_targets(&text).into_iter().take(MAX_TARGETS) {
            configs.push(RunConfig::detected(
                format!("just {recipe}"),
                format!("just {recipe}"),
            ));
        }
    }
    configs
}

/// The extension of the first top-level file with one of `extensions`.
fn dir_has_extension(root: &Path, extensions: &[&'static str]) -> Option<&'static str> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut found = None;
    for entry in entries.flatten().take(500) {
        let path = entry.path();
        let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if let Some(known) = extensions.iter().find(|known| **known == extension) {
            // A project file wins over a solution: `dotnet run` needs one.
            if found.is_none() || !matches!(*known, "sln" | "slnx") {
                found = Some(*known);
            }
        }
    }
    found
}

/// A word every shell takes as it is (no quotes, variables or operators in
/// POSIX shells, PowerShell or cmd; `@` starts a splat in PowerShell).
fn plain_word(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with('-')
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_:./+".contains(c))
}

/// `name:` at the start of a line, skipping special and pattern targets.
fn make_targets(text: &str) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim_end();
        let valid = !name.is_empty()
            && !line.starts_with([' ', '\t', '.', '#'])
            && !rest.starts_with('=')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if valid && !targets.iter().any(|known| known == name) {
            targets.push(name.to_string());
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_scripts_and_targets() {
        let root = std::env::temp_dir().join(format!("ph-runs-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts":{"lint":"eslint .","dev":"vite","build:prod":"vite build","x; rm -rf ~":"true","a';calc;'":"true","@x":"true"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(
            root.join("Makefile"),
            ".PHONY: all\nall: build\nbuild:\n\tcc x.c\nVAR := 1\n%.o: %.c\n",
        )
        .unwrap();
        let names: Vec<String> = detect(&root)
            .into_iter()
            .map(|config| config.command)
            .collect();
        assert_eq!(
            names,
            vec![
                "pnpm dev",
                "pnpm lint",
                "pnpm build:prod",
                "make all",
                "make build"
            ]
        );
    }

    fn commands(files: &[(&str, &str)]) -> Vec<String> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "ph-runs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let found = detect(&root)
            .into_iter()
            .map(|config| config.command)
            .collect();
        let _ = std::fs::remove_dir_all(&root);
        found
    }

    #[test]
    fn detects_other_build_tools() {
        let mvn = commands(&[("pom.xml", "<artifactId>spring-boot-starter</artifactId>")]);
        assert_eq!(mvn, ["mvn spring-boot:run", "mvn package", "mvn test"]);
        let dotnet = commands(&[("App.sln", ""), ("App.csproj", "")]);
        assert_eq!(dotnet, ["dotnet run", "dotnet build", "dotnet test"]);
        assert_eq!(
            commands(&[("CMakeLists.txt", "")]),
            ["cmake -S . -B build", "cmake --build build"]
        );
        assert_eq!(
            commands(&[("pubspec.yaml", "dependencies:\n  flutter:\n")]),
            ["flutter run", "flutter test"]
        );
        assert_eq!(
            commands(&[(
                "deno.jsonc",
                "{\n// tasks\n\"tasks\": {\"dev\": \"deno run main.ts\"}}"
            )]),
            ["deno task dev"]
        );
        assert_eq!(commands(&[("main.py", "")]), ["python main.py"]);
        let gradlew = if cfg!(windows) {
            "gradlew.bat"
        } else {
            "./gradlew"
        };
        let android = commands(&[("gradlew", ""), ("app/build.gradle", "")]);
        assert!(android.contains(&format!("{gradlew} assembleDebug")));
        let jvm = commands(&[
            ("gradlew", ""),
            ("build.gradle.kts", "plugins { application }"),
        ]);
        assert_eq!(jvm[0], format!("{gradlew} run"));
    }
}
