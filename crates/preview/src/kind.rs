//! What kind of file a path is, for picking its viewer.

use std::path::Path;

/// A kind of file with a viewer of its own. Text kinds are edited as text
/// too; the rest are shown only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    // Text, with a preview beside the source.
    Markdown,
    Mermaid,
    Svg,
    Json,
    JsonLines,
    Yaml,
    Toml,
    /// Comma- or tab-separated values.
    Table {
        tabs: bool,
    },
    Html,
    Diff,
    Env,
    Log,
    Notebook,
    /// Any other text: the editor alone.
    Text,
    // Shown only.
    Image,
    Audio,
    Video,
    Pdf,
    Sqlite,
    Spreadsheet,
    Archive,
    Font,
}

/// How a text file is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    /// The editor.
    Source,
    /// The rendered file.
    Preview,
    /// The editor and the rendered file side by side.
    Split,
    /// Markdown edited as formatted text.
    Rich,
}

impl View {
    pub fn label(self) -> &'static str {
        match self {
            View::Source => "Source",
            View::Preview => "Preview",
            View::Split => "Split",
            View::Rich => "Rich",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            View::Source => "source",
            View::Preview => "preview",
            View::Split => "split",
            View::Rich => "rich",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "source" => View::Source,
            "preview" => View::Preview,
            "split" => View::Split,
            "rich" => View::Rich,
            _ => return None,
        })
    }
}

impl Kind {
    /// The kind of `path`, from its name. `head` (the first bytes, when
    /// read) tells binary files from text where the name does not.
    pub fn of(path: &Path) -> Kind {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if name == ".env" || name.starts_with(".env.") || name.ends_with(".env") {
            return Kind::Env;
        }
        let extension = match name.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() => extension,
            _ => return Kind::Text,
        };
        match extension {
            "md" | "markdown" | "mdown" | "mkd" | "mdx" => Kind::Markdown,
            "mmd" | "mermaid" => Kind::Mermaid,
            "svg" => Kind::Svg,
            "json" | "jsonc" | "json5" | "geojson" | "topojson" | "har" | "webmanifest" => {
                Kind::Json
            }
            "jsonl" | "ndjson" => Kind::JsonLines,
            "yaml" | "yml" => Kind::Yaml,
            "toml" => Kind::Toml,
            "csv" => Kind::Table { tabs: false },
            "tsv" | "tab" => Kind::Table { tabs: true },
            "html" | "htm" | "xhtml" => Kind::Html,
            "diff" | "patch" => Kind::Diff,
            "log" => Kind::Log,
            "ipynb" => Kind::Notebook,
            "png" | "jpg" | "jpeg" | "jpe" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff"
            | "pbm" | "pgm" | "ppm" | "pnm" => Kind::Image,
            "mp3" | "wav" | "wave" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "aif"
            | "aiff" | "aifc" | "caf" => Kind::Audio,
            "mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi" | "wmv" | "mpg" | "mpeg" | "3gp" => {
                Kind::Video
            }
            "pdf" => Kind::Pdf,
            "db" | "sqlite" | "sqlite3" | "db3" | "s3db" | "sl3" => Kind::Sqlite,
            "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => Kind::Spreadsheet,
            "zip" | "jar" | "war" | "ear" | "apk" | "aar" | "whl" | "nupkg" | "vsix" | "xpi"
            | "tar" | "tgz" | "gz" | "crate" => Kind::Archive,
            "ttf" | "otf" | "ttc" => Kind::Font,
            _ => Kind::Text,
        }
    }

    /// Opened as text in the editor (the others are read by their viewer).
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Kind::Markdown
                | Kind::Mermaid
                | Kind::Svg
                | Kind::Json
                | Kind::JsonLines
                | Kind::Yaml
                | Kind::Toml
                | Kind::Table { .. }
                | Kind::Html
                | Kind::Diff
                | Kind::Env
                | Kind::Log
                | Kind::Notebook
                | Kind::Text
        )
    }

    /// The ways a text file of this kind can be shown, the first the
    /// default.
    pub fn views(self) -> &'static [View] {
        match self {
            Kind::Markdown => &[View::Rich, View::Source, View::Split],
            Kind::Mermaid | Kind::Svg => &[View::Split, View::Preview, View::Source],
            Kind::Notebook | Kind::Table { .. } | Kind::Env | Kind::Log => {
                &[View::Preview, View::Source, View::Split]
            }
            Kind::Json | Kind::JsonLines | Kind::Yaml | Kind::Toml | Kind::Diff => {
                &[View::Source, View::Preview, View::Split]
            }
            Kind::Html => &[View::Source],
            _ => &[],
        }
    }

    /// The name of the kind for settings (the view last picked for it).
    pub fn key(self) -> &'static str {
        match self {
            Kind::Markdown => "markdown",
            Kind::Mermaid => "mermaid",
            Kind::Svg => "svg",
            Kind::Json => "json",
            Kind::JsonLines => "jsonl",
            Kind::Yaml => "yaml",
            Kind::Toml => "toml",
            Kind::Table { .. } => "table",
            Kind::Html => "html",
            Kind::Diff => "diff",
            Kind::Env => "env",
            Kind::Log => "log",
            Kind::Notebook => "notebook",
            Kind::Text => "text",
            Kind::Image => "image",
            Kind::Audio => "audio",
            Kind::Video => "video",
            Kind::Pdf => "pdf",
            Kind::Sqlite => "sqlite",
            Kind::Spreadsheet => "spreadsheet",
            Kind::Archive => "archive",
            Kind::Font => "font",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_come_from_names() {
        let kind = |name: &str| Kind::of(Path::new(name));
        assert_eq!(kind("docs/README.md"), Kind::Markdown);
        assert_eq!(kind("flow.mmd"), Kind::Mermaid);
        assert_eq!(kind("data.TSV"), Kind::Table { tabs: true });
        assert_eq!(kind(".env"), Kind::Env);
        assert_eq!(kind(".env.local"), Kind::Env);
        assert_eq!(kind("prod.env"), Kind::Env);
        assert_eq!(kind("clip.MOV"), Kind::Video);
        assert_eq!(kind("song.flac"), Kind::Audio);
        assert_eq!(kind("app.sqlite3"), Kind::Sqlite);
        assert_eq!(kind("Makefile"), Kind::Text);
        // A dot file is a name, not an extension.
        assert_eq!(kind(".gitignore"), Kind::Text);
        assert_eq!(kind("main.rs"), Kind::Text);
    }

    #[test]
    fn text_kinds_have_their_views() {
        assert_eq!(Kind::Markdown.views()[0], View::Rich);
        assert!(Kind::Text.views().is_empty());
        assert!(Kind::Image.views().is_empty());
        assert!(Kind::Json.is_text() && !Kind::Pdf.is_text());
        for view in [View::Source, View::Preview, View::Split, View::Rich] {
            assert_eq!(View::from_key(view.key()), Some(view));
        }
    }
}
