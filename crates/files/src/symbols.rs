//! Where things are defined, found by reading source files with a few
//! patterns per language (no language server needed), for `@` mentions of
//! a function, type or class. It can miss unusual code; it never runs any.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::roots::Roots;

/// Files larger than this are skipped (generated or vendored code).
const MAX_FILE: u64 = 256 * 1024;
/// Most files read for symbols.
const MAX_FILES: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// `fn`, `struct`, `class`, `def`…
    pub kind: &'static str,
    /// As the workspace names it (`server/src/main.rs`).
    pub path: Arc<str>,
    /// 1-based.
    pub line: usize,
}

/// One pattern: the kind it finds, and a regex whose group `name` is the
/// name. Anchored at line starts (after indentation and modifiers).
struct Pattern {
    kind: &'static str,
    regex: Regex,
}

fn patterns(extension: &str) -> &'static [Pattern] {
    static RUST: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
        build(&[
            (
                "fn",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+|async\s+|unsafe\s+|extern\s+\S+\s+)*fn\s+(?P<name>\w+)",
            ),
            (
                "struct",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+(?P<name>\w+)",
            ),
            ("enum", r"^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+(?P<name>\w+)"),
            (
                "trait",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?trait\s+(?P<name>\w+)",
            ),
            ("type", r"^\s*(?:pub(?:\([^)]*\))?\s+)?type\s+(?P<name>\w+)"),
            (
                "const",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+(?P<name>[A-Z_][A-Z0-9_]*)\s*:",
            ),
            ("macro", r"^\s*macro_rules!\s+(?P<name>\w+)"),
        ])
    });
    static SCRIPT: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
        build(&[
            (
                "function",
                r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\*?\s+(?P<name>[\w$]+)",
            ),
            (
                "class",
                r"^\s*(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+(?P<name>[\w$]+)",
            ),
            (
                "interface",
                r"^\s*(?:export\s+)?interface\s+(?P<name>[\w$]+)",
            ),
            (
                "type",
                r"^\s*(?:export\s+)?type\s+(?P<name>[\w$]+)\s*(?:<[^=]*>)?\s*=",
            ),
            (
                "const",
                r"^\s*(?:export\s+)?const\s+(?P<name>[\w$]+)\s*(?::[^=]+)?=\s*(?:async\s+)?(?:\([^)]*\)|\w+)\s*=>",
            ),
            // A method in a class body: `  async save(a: A): B {`.
            (
                "method",
                r"^\s+(?:(?:public|private|protected|static|async|readonly|override|get|set)\s+)*(?P<name>[\w$]+)\s*(?:<[^>]*>)?\s*\([^)]*\)\s*(?::[^{]+)?\{\s*$",
            ),
        ])
    });
    static PYTHON: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
        build(&[
            ("def", r"^\s*(?:async\s+)?def\s+(?P<name>\w+)"),
            ("class", r"^\s*class\s+(?P<name>\w+)"),
        ])
    });
    static GO: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
        build(&[
            ("func", r"^func\s+(?:\([^)]*\)\s*)?(?P<name>\w+)"),
            ("type", r"^type\s+(?P<name>\w+)"),
        ])
    });
    static JVM: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
        build(&[
            (
                "class",
                r"^\s*(?:(?:public|private|protected|internal|abstract|final|open|sealed|data|static)\s+)*(?:class|interface|enum|object|record)\s+(?P<name>\w+)",
            ),
            (
                "fun",
                r"^\s*(?:(?:public|private|protected|internal|override|suspend|inline|open)\s+)*fun\s+(?:<[^>]*>\s*)?(?:\w+\.)?(?P<name>\w+)",
            ),
            (
                "func",
                r"^\s*(?:(?:public|private|fileprivate|internal|open|static|override|mutating)\s+)*func\s+(?P<name>\w+)",
            ),
            (
                "struct",
                r"^\s*(?:(?:public|private|internal)\s+)*struct\s+(?P<name>\w+)",
            ),
            // Java and C# methods: modifiers, a type, the name, `(`.
            (
                "method",
                r"^\s*(?:(?:public|private|protected|internal|static|final|abstract|synchronized|override|virtual|async|sealed|partial|extern|unsafe|default)\s+)+(?:<[^>]*>\s*)?[\w<>\[\],.?]+\s+(?P<name>\w+)\s*\(",
            ),
        ])
    });
    static C: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
        build(&[
            (
                "struct",
                r"^\s*(?:typedef\s+)?(?:struct|class|union|enum)\s+(?P<name>\w+)\s*(?:[:{]|$)",
            ),
            (
                "fn",
                r"^[A-Za-z_][\w\s\*&:<>,]*?\b(?P<name>[A-Za-z_]\w*)\s*\([^;]*$",
            ),
        ])
    });
    static NONE: LazyLock<Vec<Pattern>> = LazyLock::new(Vec::new);
    match extension {
        "rs" => &RUST,
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" => &SCRIPT,
        "py" => &PYTHON,
        "go" => &GO,
        "java" | "kt" | "kts" | "scala" | "swift" | "cs" | "dart" => &JVM,
        "c" | "h" | "cc" | "cpp" | "hpp" | "cxx" | "m" | "mm" => &C,
        _ => &NONE,
    }
}

/// Whether files with `extension` have definitions this module finds.
pub fn has_patterns(extension: &str) -> bool {
    !patterns(extension).is_empty()
}

fn build(specs: &[(&'static str, &str)]) -> Vec<Pattern> {
    specs
        .iter()
        .map(|(kind, pattern)| Pattern {
            kind,
            regex: Regex::new(pattern).expect("valid symbol pattern"),
        })
        .collect()
}

/// The definitions in one file's text.
pub fn scan_text(path: &Arc<str>, extension: &str, text: &str) -> Vec<Symbol> {
    let patterns = patterns(extension);
    if patterns.is_empty() {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (ix, line) in text.lines().enumerate() {
        // Comments and control flow that look like calls are not definitions.
        let trimmed = line.trim_start();
        if trimmed.starts_with("//")
            || trimmed.starts_with('#') && extension != "py"
            || trimmed.starts_with('*')
        {
            continue;
        }
        if let Some((kind, name)) = patterns.iter().find_map(|pattern| {
            let caps = pattern.regex.captures(line)?;
            Some((pattern.kind, caps.name("name")?.as_str()))
        }) && !matches!(
            name,
            "if" | "for"
                | "while"
                | "switch"
                | "return"
                | "catch"
                | "main"
                | "new"
                | "else"
                | "do"
                | "try"
                | "function"
                | "constructor"
        ) {
            found.push(Symbol {
                name: name.to_string(),
                kind,
                path: path.clone(),
                line: ix + 1,
            });
        }
    }
    found
}

/// Every definition in the workspace's files (`files`: their paths as the
/// workspace names them, from [`Roots::index`]).
pub fn index(roots: &Roots, files: &[Arc<str>]) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for file in files.iter().take(MAX_FILES) {
        let extension = Path::new(file.as_ref())
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default();
        if patterns(extension).is_empty() {
            continue;
        }
        let Some(path) = roots.absolute(file) else {
            continue;
        };
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_FILE) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            symbols.extend(scan_text(file, extension, &text));
        }
    }
    symbols
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(extension: &str, text: &str) -> Vec<(String, &'static str, usize)> {
        let path: Arc<str> = Arc::from("x");
        scan_text(&path, extension, text)
            .into_iter()
            .map(|symbol| (symbol.name, symbol.kind, symbol.line))
            .collect()
    }

    #[test]
    fn finds_definitions_in_common_languages() {
        let rust = "pub struct Store {}\nimpl Store {\n    pub async fn open(&self) {}\n}\n// fn not_this() {}\nconst MAX: usize = 3;\n";
        assert_eq!(
            names("rs", rust),
            [
                ("Store".into(), "struct", 1),
                ("open".into(), "fn", 3),
                ("MAX".into(), "const", 6)
            ]
        );
        let ts = "export default class App {}\nexport const useThing = (a) => a;\nfunction helper() {}\nif (x) {}\n";
        let found: Vec<String> = names("ts", ts)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(found, ["App", "useThing", "helper"]);
        assert_eq!(
            names("py", "class Model:\n    def save(self):\n")[1].0,
            "save"
        );
        assert_eq!(
            names("go", "func (s *Server) Serve() {}\ntype Server struct{}\n")[0].0,
            "Serve"
        );
        assert!(names("md", "fn nope()").is_empty(), "only source files");
        let java = "public class Repo {\n    public static List<User> findAll(int limit) {\n        if (x) {\n        return load(a);\n    }\n}\n";
        let found: Vec<String> = names("java", java)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(found, ["Repo", "findAll"]);
        let cs = "internal sealed class Cart {\n    public async Task<int> Total() {\n";
        assert_eq!(names("cs", cs)[1].0, "Total");
        let class =
            "class Api {\n  async fetchUser(id: string): Promise<User> {\n    if (id) {\n  }\n}\n";
        let found: Vec<String> = names("ts", class)
            .into_iter()
            .map(|(name, _, _)| name)
            .collect();
        assert_eq!(found, ["Api", "fetchUser"]);
    }
}
