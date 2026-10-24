//! Which language server handles which files. A server is used when its
//! program is on the user's `PATH`; nothing is downloaded.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerSpec {
    /// Shown to the user, and the key for one running server.
    pub name: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// The LSP `languageId` for documents it opens.
    pub language: &'static str,
    pub extensions: &'static [&'static str],
}

/// Known servers, in order of preference per language.
pub const SERVERS: &[ServerSpec] = &[
    ServerSpec {
        name: "rust-analyzer",
        program: "rust-analyzer",
        args: &[],
        language: "rust",
        extensions: &["rs"],
    },
    ServerSpec {
        name: "TypeScript",
        program: "typescript-language-server",
        args: &["--stdio"],
        language: "typescript",
        extensions: &["ts", "tsx", "mts", "cts"],
    },
    ServerSpec {
        name: "TypeScript",
        program: "typescript-language-server",
        args: &["--stdio"],
        language: "javascript",
        extensions: &["js", "jsx", "mjs", "cjs"],
    },
    ServerSpec {
        name: "Pyright",
        program: "pyright-langserver",
        args: &["--stdio"],
        language: "python",
        extensions: &["py", "pyi"],
    },
    ServerSpec {
        name: "Python LSP",
        program: "pylsp",
        args: &[],
        language: "python",
        extensions: &["py", "pyi"],
    },
    ServerSpec {
        name: "gopls",
        program: "gopls",
        args: &[],
        language: "go",
        extensions: &["go"],
    },
    ServerSpec {
        name: "clangd",
        program: "clangd",
        args: &[],
        language: "cpp",
        extensions: &["c", "h", "cc", "cpp", "cxx", "hpp", "hh", "m", "mm"],
    },
    ServerSpec {
        name: "Kotlin",
        program: "kotlin-language-server",
        args: &[],
        language: "kotlin",
        extensions: &["kt", "kts"],
    },
    ServerSpec {
        name: "Java",
        program: "jdtls",
        args: &[],
        language: "java",
        extensions: &["java"],
    },
    ServerSpec {
        name: "C#",
        program: "csharp-ls",
        args: &[],
        language: "csharp",
        extensions: &["cs"],
    },
    ServerSpec {
        name: "Swift",
        program: "sourcekit-lsp",
        args: &[],
        language: "swift",
        extensions: &["swift"],
    },
    ServerSpec {
        name: "Dart",
        program: "dart",
        args: &["language-server", "--protocol=lsp"],
        language: "dart",
        extensions: &["dart"],
    },
    ServerSpec {
        name: "Zig",
        program: "zls",
        args: &[],
        language: "zig",
        extensions: &["zig"],
    },
    ServerSpec {
        name: "Lua",
        program: "lua-language-server",
        args: &[],
        language: "lua",
        extensions: &["lua"],
    },
    ServerSpec {
        name: "Ruby",
        program: "ruby-lsp",
        args: &[],
        language: "ruby",
        extensions: &["rb"],
    },
    ServerSpec {
        name: "Bash",
        program: "bash-language-server",
        args: &["start"],
        language: "shellscript",
        extensions: &["sh", "bash"],
    },
    ServerSpec {
        name: "JSON",
        program: "vscode-json-language-server",
        args: &["--stdio"],
        language: "json",
        extensions: &["json", "jsonc"],
    },
    ServerSpec {
        name: "YAML",
        program: "yaml-language-server",
        args: &["--stdio"],
        language: "yaml",
        extensions: &["yaml", "yml"],
    },
    ServerSpec {
        name: "HTML",
        program: "vscode-html-language-server",
        args: &["--stdio"],
        language: "html",
        extensions: &["html", "htm"],
    },
    ServerSpec {
        name: "CSS",
        program: "vscode-css-language-server",
        args: &["--stdio"],
        language: "css",
        extensions: &["css", "scss", "less"],
    },
];

/// The candidates for `path`, preferred first.
pub fn candidates(path: &Path) -> impl Iterator<Item = &'static ServerSpec> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    SERVERS
        .iter()
        .filter(move |spec| spec.extensions.contains(&extension.as_str()))
}

/// The first installed server for `path`, with its program.
pub fn find(path: &Path) -> Option<(&'static ServerSpec, PathBuf)> {
    candidates(path).find_map(|spec| ph_env::which(spec.program).map(|program| (spec, program)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_by_extension() {
        let names: Vec<_> = candidates(Path::new("src/App.TSX"))
            .map(|s| s.language)
            .collect();
        assert_eq!(names, ["typescript"]);
        let python: Vec<_> = candidates(Path::new("a.py")).map(|s| s.name).collect();
        assert_eq!(python, ["Pyright", "Python LSP"]);
        assert_eq!(candidates(Path::new("notes.txt")).count(), 0);
    }
}
