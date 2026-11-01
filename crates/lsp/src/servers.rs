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
    /// How to install it, shown when it is missing or fails.
    pub install: &'static str,
    /// Whether it may start in a folder of someone else's code: it runs
    /// none of the project's own programs (build scripts, package
    /// manifests, its copy of a compiler service).
    pub safe_untrusted: bool,
}

/// Known servers, in order of preference per language.
pub const SERVERS: &[ServerSpec] = &[
    ServerSpec {
        name: "rust-analyzer",
        program: "rust-analyzer",
        args: &[],
        language: "rust",
        install: "rustup component add rust-analyzer",
        extensions: &["rs"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "TypeScript",
        program: "typescript-language-server",
        args: &["--stdio"],
        language: "typescript",
        install: "npm install -g typescript typescript-language-server",
        extensions: &["ts", "tsx", "mts", "cts"],
        safe_untrusted: false,
    },
    ServerSpec {
        name: "TypeScript",
        program: "typescript-language-server",
        args: &["--stdio"],
        language: "javascript",
        install: "npm install -g typescript typescript-language-server",
        extensions: &["js", "jsx", "mjs", "cjs"],
        safe_untrusted: false,
    },
    ServerSpec {
        name: "Pyright",
        program: "pyright-langserver",
        args: &["--stdio"],
        language: "python",
        install: "npm install -g pyright",
        extensions: &["py", "pyi"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "Python LSP",
        program: "pylsp",
        args: &[],
        language: "python",
        install: "pip install python-lsp-server",
        extensions: &["py", "pyi"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "gopls",
        program: "gopls",
        args: &[],
        language: "go",
        install: "go install golang.org/x/tools/gopls@latest",
        extensions: &["go"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "clangd",
        program: "clangd",
        args: &[],
        language: "cpp",
        install: "Install clangd (LLVM): brew install llvm, apt install clangd, or winget install LLVM.LLVM",
        extensions: &["c", "h", "cc", "cpp", "cxx", "hpp", "hh", "m", "mm"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "Kotlin",
        program: "kotlin-language-server",
        args: &[],
        language: "kotlin",
        install: "brew install kotlin-language-server (or the release from github.com/fwcd/kotlin-language-server)",
        extensions: &["kt", "kts"],
        safe_untrusted: false,
    },
    ServerSpec {
        name: "Java",
        program: "jdtls",
        args: &[],
        language: "java",
        install: "brew install jdtls (Eclipse JDT Language Server)",
        extensions: &["java"],
        safe_untrusted: false,
    },
    ServerSpec {
        name: "C#",
        program: "csharp-ls",
        args: &[],
        language: "csharp",
        install: "dotnet tool install --global csharp-ls",
        extensions: &["cs"],
        safe_untrusted: false,
    },
    ServerSpec {
        name: "Swift",
        program: "sourcekit-lsp",
        args: &[],
        language: "swift",
        install: "Comes with Xcode or the Swift toolchain (sourcekit-lsp)",
        extensions: &["swift"],
        safe_untrusted: false,
    },
    ServerSpec {
        name: "Dart",
        program: "dart",
        args: &["language-server", "--protocol=lsp"],
        language: "dart",
        install: "Comes with the Dart and Flutter SDKs",
        extensions: &["dart"],
        safe_untrusted: false,
    },
    ServerSpec {
        name: "Zig",
        program: "zls",
        args: &[],
        language: "zig",
        install: "brew install zls",
        extensions: &["zig"],
        safe_untrusted: false,
    },
    ServerSpec {
        name: "Lua",
        program: "lua-language-server",
        args: &[],
        language: "lua",
        install: "brew install lua-language-server",
        extensions: &["lua"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "Ruby",
        program: "ruby-lsp",
        args: &[],
        language: "ruby",
        install: "gem install ruby-lsp",
        extensions: &["rb"],
        safe_untrusted: false,
    },
    ServerSpec {
        name: "Bash",
        program: "bash-language-server",
        args: &["start"],
        language: "shellscript",
        install: "npm install -g bash-language-server",
        extensions: &["sh", "bash"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "JSON",
        program: "vscode-json-language-server",
        args: &["--stdio"],
        language: "json",
        install: "npm install -g vscode-langservers-extracted",
        extensions: &["json", "jsonc"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "YAML",
        program: "yaml-language-server",
        args: &["--stdio"],
        language: "yaml",
        install: "npm install -g yaml-language-server",
        extensions: &["yaml", "yml"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "HTML",
        program: "vscode-html-language-server",
        args: &["--stdio"],
        language: "html",
        install: "npm install -g vscode-langservers-extracted",
        extensions: &["html", "htm"],
        safe_untrusted: true,
    },
    ServerSpec {
        name: "CSS",
        program: "vscode-css-language-server",
        args: &["--stdio"],
        language: "css",
        install: "npm install -g vscode-langservers-extracted",
        extensions: &["css", "scss", "less"],
        safe_untrusted: true,
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
