//! Structured data as a tree of [`Node`]s, keys in the order written: JSON
//! (with comments, JSONC/JSON5), JSON Lines, YAML and TOML. And formatting
//! JSON: pretty or minified, keeping key order and number spelling.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Null,
    Bool(bool),
    /// As written (`1e3` stays `1e3`).
    Number(String),
    String(String),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a value")
    }
    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_none<E>(self) -> Result<Node, E> {
        Ok(Node::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Node, D::Error> {
        Node::deserialize(deserializer)
    }
    fn visit_bool<E>(self, value: bool) -> Result<Node, E> {
        Ok(Node::Bool(value))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Node, E> {
        Ok(Node::Number(value.to_string()))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Node, E> {
        Ok(Node::Number(value.to_string()))
    }
    fn visit_f64<E>(self, value: f64) -> Result<Node, E> {
        Ok(Node::Number(value.to_string()))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Node, E> {
        Ok(Node::String(value.to_string()))
    }
    fn visit_string<E>(self, value: String) -> Result<Node, E> {
        Ok(Node::String(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Node::Array(items))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        let mut entries = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, Node>()? {
            entries.push((key, value));
        }
        Ok(Node::Object(entries))
    }
}

/// Parses JSON; comments and trailing commas (JSONC, JSON5) too.
pub fn parse_json(text: &str) -> Result<Node, String> {
    match serde_json::from_str(text) {
        Ok(node) => Ok(node),
        Err(strict) => json5::from_str(text).map_err(|_| describe_json_error(&strict)),
    }
}

/// One value per line; a line that is not JSON says why.
pub fn parse_json_lines(text: &str) -> Node {
    Node::Array(
        text.lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(ix, line)| {
                serde_json::from_str(line).unwrap_or_else(|err| {
                    Node::String(format!("line {}: {}", ix + 1, describe_json_error(&err)))
                })
            })
            .collect(),
    )
}

pub fn parse_toml(text: &str) -> Result<Node, String> {
    toml::from_str(text).map_err(|err| err.message().to_string())
}

/// YAML; a file of several documents is a list of them.
pub fn parse_yaml(text: &str) -> Result<Node, String> {
    let docs = yaml_rust2::YamlLoader::load_from_str(text).map_err(|err| err.to_string())?;
    let mut nodes: Vec<Node> = docs.iter().map(from_yaml).collect();
    Ok(match nodes.len() {
        0 => Node::Null,
        1 => nodes.remove(0),
        _ => Node::Array(nodes),
    })
}

fn from_yaml(yaml: &yaml_rust2::Yaml) -> Node {
    use yaml_rust2::Yaml;
    match yaml {
        Yaml::Real(text) => Node::Number(text.clone()),
        Yaml::Integer(value) => Node::Number(value.to_string()),
        Yaml::String(text) => Node::String(text.clone()),
        Yaml::Boolean(value) => Node::Bool(*value),
        Yaml::Array(items) => Node::Array(items.iter().map(from_yaml).collect()),
        Yaml::Hash(map) => Node::Object(
            map.iter()
                .map(|(key, value)| (yaml_key(key), from_yaml(value)))
                .collect(),
        ),
        Yaml::Alias(_) | Yaml::Null | Yaml::BadValue => Node::Null,
    }
}

fn yaml_key(key: &yaml_rust2::Yaml) -> String {
    use yaml_rust2::Yaml;
    match key {
        Yaml::String(text) | Yaml::Real(text) => text.clone(),
        Yaml::Integer(value) => value.to_string(),
        Yaml::Boolean(value) => value.to_string(),
        Yaml::Null => "~".into(),
        other => format!("{other:?}"),
    }
}

fn describe_json_error(err: &serde_json::Error) -> String {
    format!("{err}")
}

/// Pretty JSON, `indent` spaces deep; refuses text that is not strict JSON
/// (formatting would lose comments).
pub fn format(text: &str, indent: usize) -> Result<String, String> {
    check(text)?;
    Ok(reflow(text, Some(indent)))
}

/// JSON on one line.
pub fn minify(text: &str) -> Result<String, String> {
    check(text)?;
    Ok(reflow(text, None))
}

fn check(text: &str) -> Result<(), String> {
    serde_json::from_str::<de::IgnoredAny>(text)
        .map(|_| ())
        .map_err(|err| describe_json_error(&err))
}

/// Re-spaces valid JSON token by token: strings and numbers stay as
/// written, so nothing changes but whitespace.
fn reflow(text: &str, indent: Option<usize>) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut depth = 0usize;
    let mut chars = text.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        if let Some(indent) = indent {
            out.push('\n');
            out.extend(std::iter::repeat_n(' ', indent * depth));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push('"');
                while let Some(c) = chars.next() {
                    out.push(c);
                    match c {
                        '\\' => out.extend(chars.next()),
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '{' | '[' => {
                out.push(c);
                // An empty object or array stays on one line.
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
                let close = if c == '{' { '}' } else { ']' };
                if chars.peek() == Some(&close) {
                    out.push(chars.next().unwrap_or(close));
                } else {
                    depth += 1;
                    newline(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(',');
                newline(&mut out, depth);
            }
            ':' => {
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
            }
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    if indent.is_some() {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_key_order_and_numbers() {
        let node = parse_json(r#"{"z": 1e3, "a": [true, null, "x"]}"#).unwrap();
        let Node::Object(entries) = node else {
            panic!("an object")
        };
        assert_eq!(entries[0].0, "z");
        assert_eq!(entries[1].0, "a");
        assert!(matches!(&entries[1].1, Node::Array(items) if items.len() == 3));
    }

    #[test]
    fn reads_comments_yaml_toml_and_lines() {
        assert!(parse_json("{ // note\n \"a\": 1, }").is_ok());
        assert!(parse_json("{ broken").is_err());
        let yaml = parse_yaml("name: app\nports:\n  - 80\n  - 443\n").unwrap();
        assert!(matches!(&yaml, Node::Object(entries) if entries[1].0 == "ports"));
        let toml = parse_toml("[package]\nname = \"x\"\nversion = \"1\"\n").unwrap();
        let Node::Object(top) = toml else { panic!() };
        let Node::Object(package) = &top[0].1 else {
            panic!()
        };
        assert_eq!(package[0].0, "name");
        let lines = parse_json_lines("{\"a\":1}\n\nnot json\n");
        assert!(matches!(&lines, Node::Array(items) if items.len() == 2));
    }

    #[test]
    fn formats_and_minifies_only_whitespace() {
        let text = r#"{"b":[1,2,{}],"a":{"s":"x, y: \"z\""},"n":1.50}"#;
        let pretty = format(text, 2).unwrap();
        assert_eq!(
            pretty,
            "{\n  \"b\": [\n    1,\n    2,\n    {}\n  ],\n  \"a\": {\n    \"s\": \"x, y: \\\"z\\\"\"\n  },\n  \"n\": 1.50\n}\n"
        );
        assert_eq!(minify(&pretty).unwrap(), text);
        assert!(format("{\"a\": 1, // c\n}", 2).is_err());
        assert_eq!(format("[ ]", 2).unwrap(), "[]\n");
    }
}
