//! Source maps (version 3): where a place in a built script came from in
//! the original sources, so an error's stack names the workspace's files
//! (`src/App.tsx:42:7`) rather than a bundle's line and column.

use serde_json::Value;

/// One mapping: generated column, source, original line and column (all
/// zero-based).
type Segment = (u32, u32, u32, u32);

/// A decoded source map.
pub struct SourceMap {
    sources: Vec<String>,
    /// Per generated line, its segments sorted by generated column.
    lines: Vec<Vec<Segment>>,
}

/// An original place: the source as the map names it, 1-based line and
/// column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Original {
    pub source: String,
    pub line: u32,
    pub column: u32,
}

impl SourceMap {
    /// Parses a map's JSON; `None` when it is not a usable version 3 map
    /// (index maps with `sections` are not read).
    pub fn parse(text: &str) -> Option<Self> {
        let map: Value = serde_json::from_str(text).ok()?;
        if map["version"].as_u64() != Some(3) {
            return None;
        }
        let root = map["sourceRoot"].as_str().unwrap_or_default();
        let sources = map["sources"]
            .as_array()?
            .iter()
            .map(|source| {
                let source = source.as_str().unwrap_or_default();
                if root.is_empty() || source.contains("://") || source.starts_with('/') {
                    source.to_string()
                } else {
                    format!("{}/{}", root.trim_end_matches('/'), source)
                }
            })
            .collect();
        let lines = decode(map["mappings"].as_str()?)?;
        Some(Self { sources, lines })
    }

    /// Where 1-based `line` and `column` of the built script came from.
    pub fn lookup(&self, line: u32, column: u32) -> Option<Original> {
        let segments = self.lines.get(line.checked_sub(1)? as usize)?;
        let column = column.saturating_sub(1);
        let index = segments.partition_point(|segment| segment.0 <= column);
        let segment = segments.get(index.checked_sub(1)?)?;
        Some(Original {
            source: self.sources.get(segment.1 as usize)?.clone(),
            line: segment.2.saturating_add(1),
            column: segment.3.saturating_add(1),
        })
    }
}

/// The `mappings` field: base64 VLQ segments, `,` between segments and
/// `;` between generated lines; fields relative to the previous segment.
/// The page wrote the map, so sums that overflow make it unusable (`None`)
/// rather than a panic.
fn decode(mappings: &str) -> Option<Vec<Vec<Segment>>> {
    let mut lines = Vec::new();
    let (mut source, mut line, mut column) = (0i64, 0i64, 0i64);
    for text in mappings.split(';') {
        let mut segments = Vec::new();
        let mut generated = 0i64;
        for segment in text.split(',').filter(|segment| !segment.is_empty()) {
            let fields = vlq(segment)?;
            generated = generated.checked_add(*fields.first()?)?;
            if fields.len() >= 4 {
                source = source.checked_add(fields[1])?;
                line = line.checked_add(fields[2])?;
                column = column.checked_add(fields[3])?;
                // Negative or past `u32` places name nothing real: skipped.
                if let (Ok(generated), Ok(source), Ok(line), Ok(column)) = (
                    u32::try_from(generated),
                    u32::try_from(source),
                    u32::try_from(line),
                    u32::try_from(column),
                ) {
                    segments.push((generated, source, line, column));
                }
            }
        }
        segments.sort_by_key(|segment| segment.0);
        lines.push(segments);
    }
    Some(lines)
}

fn vlq(segment: &str) -> Option<Vec<i64>> {
    let mut values = Vec::new();
    let (mut value, mut shift) = (0i64, 0u32);
    for byte in segment.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as i64;
        value += (digit & 31) << shift;
        if digit & 32 != 0 {
            shift += 5;
            // Source map values are 32-bit: seven digits at most, which
            // keeps `value` far from overflowing.
            if shift > 30 {
                return None;
            }
            continue;
        }
        let negative = value & 1 == 1;
        let magnitude = value >> 1;
        values.push(if negative { -magnitude } else { magnitude });
        value = 0;
        shift = 0;
    }
    (shift == 0).then_some(values)
}

/// A source path as bundlers write it, made relative to the project:
/// `webpack://app/./src/App.tsx` → `src/App.tsx`, `/@fs/abs/x.ts` →
/// `/abs/x.ts`, `../src/a.ts` → `src/a.ts`.
pub fn tidy_source(source: &str) -> String {
    let mut path = source;
    if let Some(rest) = path.strip_prefix("webpack://") {
        // `webpack://<name>/./src/x` or `webpack:///./src/x`.
        path = rest.split_once('/').map_or(rest, |(_, rest)| rest);
    } else if let Some(rest) = path.strip_prefix("/@fs") {
        return rest.to_string();
    } else if let Some((_, rest)) = path.split_once("://") {
        path = rest.split_once('/').map_or(rest, |(_, rest)| rest);
    }
    let mut path = path.trim_start_matches("./");
    while let Some(rest) = path.strip_prefix("../") {
        path = rest;
    }
    path.trim_start_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_looks_up() {
        // Generated line 1: col 0 → a.js 1:0; col 10 → a.js 2:4.
        // Generated line 2: col 2 → b.ts 10:0.
        let map = SourceMap::parse(
            r#"{"version":3,"sources":["a.js","b.ts"],"sourceRoot":"src","mappings":"AAAA,UACI;ECQJ"}"#,
        )
        .unwrap();
        assert_eq!(
            map.lookup(1, 1),
            Some(Original {
                source: "src/a.js".into(),
                line: 1,
                column: 1
            })
        );
        assert_eq!(
            map.lookup(1, 15),
            Some(Original {
                source: "src/a.js".into(),
                line: 2,
                column: 5
            })
        );
        assert_eq!(map.lookup(2, 1), None, "before the line's first segment");
        assert_eq!(
            map.lookup(2, 3).map(|found| (found.source, found.line)),
            Some(("src/b.ts".into(), 10))
        );
        assert_eq!(map.lookup(9, 1), None);
        assert!(SourceMap::parse(r#"{"version":2}"#).is_none());
        assert_eq!(vlq("D"), Some(vec![-1]));
        assert_eq!(vlq("2H"), Some(vec![123]));
    }

    #[test]
    fn hostile_maps_neither_panic_nor_wrap() {
        // 2^33, past `u32`: its segment is skipped, not truncated.
        assert_eq!(vlq("ggggggQ"), Some(vec![1 << 33]));
        let map =
            SourceMap::parse(r#"{"version":3,"sources":["a.js"],"mappings":"ggggggQ,AAAA;AAAA"}"#)
                .unwrap();
        assert_eq!(map.lookup(1, 1), None);
        assert!(map.lookup(2, 1).is_some());
        // Too many digits for a 32-bit value.
        assert_eq!(vlq("gggggggA"), None);
        // The largest line a map can name still reads.
        let largest = SourceMap {
            sources: vec!["a.js".into()],
            lines: vec![vec![(0, 0, u32::MAX, u32::MAX)]],
        };
        assert_eq!(largest.lookup(1, 1).map(|found| found.line), Some(u32::MAX));
    }

    #[test]
    fn sources_become_project_paths() {
        assert_eq!(tidy_source("webpack://app/./src/App.tsx"), "src/App.tsx");
        assert_eq!(tidy_source("webpack:///./src/App.tsx"), "src/App.tsx");
        assert_eq!(tidy_source("../../src/main.ts"), "src/main.ts");
        assert_eq!(
            tidy_source("http://localhost:5173/src/main.ts"),
            "src/main.ts"
        );
        assert_eq!(tidy_source("/@fs/Users/me/app/x.ts"), "/Users/me/app/x.ts");
    }
}
