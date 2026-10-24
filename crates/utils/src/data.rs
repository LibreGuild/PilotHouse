//! Data formats: JSON, TOML, YAML, XML and CSV.

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use crate::Input;

/// Format, minify, or turn into TOML.
pub fn json(input: &Input) -> Result<String> {
    let value: Value = serde_json::from_str(input.text.trim())
        .map_err(|err| anyhow::anyhow!("Not valid JSON: {err}"))?;
    Ok(match input.mode {
        0 => serde_json::to_string_pretty(&value)?,
        1 => serde_json::to_string(&value)?,
        _ => {
            if !value.is_object() {
                bail!("TOML needs an object at the top level.");
            }
            ::toml::to_string_pretty(&value).context("TOML cannot hold this (e.g. null values)")?
        }
    })
}

/// To JSON, or formatted.
pub fn toml(input: &Input) -> Result<String> {
    let value: ::toml::Table = input
        .text
        .parse()
        .map_err(|err: ::toml::de::Error| anyhow::anyhow!("Not valid TOML: {}", err.message()))?;
    Ok(match input.mode {
        0 => serde_json::to_string_pretty(&value)?,
        _ => ::toml::to_string_pretty(&value)?,
    })
}

/// To JSON, formatted, or JSON to YAML. Several documents (`---`) become a
/// JSON array.
pub fn yaml(input: &Input) -> Result<String> {
    use yaml_rust2::{Yaml, YamlLoader};
    if input.mode == 2 {
        let value: Value = serde_json::from_str(input.text.trim())
            .map_err(|err| anyhow::anyhow!("Not valid JSON: {err}"))?;
        return emit_yaml(&yaml_of(&value));
    }
    let docs = YamlLoader::load_from_str(&input.text)
        .map_err(|err| anyhow::anyhow!("Not valid YAML: {err}"))?;
    if input.mode == 1 {
        let parts: Result<Vec<String>> = docs.iter().map(emit_yaml).collect();
        return Ok(parts?.join("\n---\n"));
    }
    let value = match docs.as_slice() {
        [] => Value::Null,
        [one] => json_of(one)?,
        many => Value::Array(many.iter().map(json_of).collect::<Result<_>>()?),
    };
    return Ok(serde_json::to_string_pretty(&value)?);

    fn json_of(yaml: &Yaml) -> Result<Value> {
        Ok(match yaml {
            Yaml::Null | Yaml::BadValue => Value::Null,
            Yaml::Boolean(value) => Value::Bool(*value),
            Yaml::Integer(value) => Value::from(*value),
            Yaml::Real(text) => match text
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
            {
                Some(number) => Value::Number(number),
                None => Value::String(text.clone()),
            },
            Yaml::String(text) => Value::String(text.clone()),
            Yaml::Array(items) => Value::Array(items.iter().map(json_of).collect::<Result<_>>()?),
            Yaml::Hash(map) => {
                let mut object = serde_json::Map::new();
                for (key, value) in map {
                    let key = match key {
                        Yaml::String(text) | Yaml::Real(text) => text.clone(),
                        Yaml::Integer(number) => number.to_string(),
                        Yaml::Boolean(value) => value.to_string(),
                        Yaml::Null => "null".to_string(),
                        _ => bail!("JSON keys must be text; this YAML has a list or map as a key."),
                    };
                    object.insert(key, json_of(value)?);
                }
                Value::Object(object)
            }
            Yaml::Alias(_) => bail!("YAML aliases (*name) are not supported."),
        })
    }
}

fn yaml_of(value: &Value) -> yaml_rust2::Yaml {
    use yaml_rust2::Yaml;
    match value {
        Value::Null => Yaml::Null,
        Value::Bool(value) => Yaml::Boolean(*value),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => Yaml::Integer(integer),
            None => Yaml::Real(number.to_string()),
        },
        Value::String(text) => Yaml::String(text.clone()),
        Value::Array(items) => Yaml::Array(items.iter().map(yaml_of).collect()),
        Value::Object(object) => Yaml::Hash(
            object
                .iter()
                .map(|(key, value)| (Yaml::String(key.clone()), yaml_of(value)))
                .collect(),
        ),
    }
}

fn emit_yaml(doc: &yaml_rust2::Yaml) -> Result<String> {
    let mut out = String::new();
    yaml_rust2::YamlEmitter::new(&mut out)
        .dump(doc)
        .map_err(|err| anyhow::anyhow!("Cannot write YAML: {err}"))?;
    Ok(out.strip_prefix("---\n").unwrap_or(&out).to_string())
}

/// Formatted with two-space indents, or minified; refuses malformed XML.
pub fn xml(input: &Input) -> Result<String> {
    roxmltree::Document::parse(input.text.trim())
        .map_err(|err| anyhow::anyhow!("Not valid XML: {err}"))?;
    let mut reader = quick_xml::Reader::from_str(input.text.trim());
    reader.config_mut().trim_text(true);
    let mut writer = if input.mode == 0 {
        quick_xml::Writer::new_with_indent(Vec::new(), b' ', 2)
    } else {
        quick_xml::Writer::new(Vec::new())
    };
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Eof => break,
            event => writer.write_event(event)?,
        }
    }
    Ok(String::from_utf8(writer.into_inner())?)
}

/// CSV (or TSV: tabs are noticed) to JSON objects by the header, or to an
/// aligned table.
pub fn csv(input: &Input) -> Result<String> {
    let rows = parse_csv(&input.text);
    let Some((header, records)) = rows.split_first() else {
        bail!("There is no header row.");
    };
    if input.mode == 0 {
        let objects: Vec<Value> = records
            .iter()
            .map(|record| {
                Value::Object(
                    header
                        .iter()
                        .enumerate()
                        .map(|(ix, key)| {
                            (
                                key.clone(),
                                Value::String(record.get(ix).cloned().unwrap_or_default()),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        return Ok(serde_json::to_string_pretty(&objects)?);
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .map(|row| row.get(column).map_or(0, |cell| cell.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |row: &Vec<String>| {
        (0..columns)
            .map(|column| {
                let cell = row.get(column).map_or("", String::as_str);
                format!("{cell:<width$}", width = widths[column])
            })
            .collect::<Vec<_>>()
            .join(" | ")
            .trim_end()
            .to_string()
    };
    let mut out = vec![line(header)];
    out.push(
        widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join("-+-"),
    );
    out.extend(records.iter().map(line));
    Ok(out.join("\n"))
}

/// Rows of cells; quotes may hold the separator, newlines and `""`.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let separator = if text
        .lines()
        .next()
        .is_some_and(|line| line.contains('\t') && !line.contains(','))
    {
        '\t'
    } else {
        ','
    };
    let (mut rows, mut row, mut cell) = (Vec::new(), Vec::new(), String::new());
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == separator && !quoted => row.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut row));
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows.retain(|row| !(row.len() == 1 && row[0].trim().is_empty()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_goes_to_json_and_back() {
        let yaml = "name: app\nports: [80, 443]\nratio: 0.5\nenabled: true\n";
        let json = super::yaml(&Input::new(yaml)).unwrap();
        let value: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["ports"][1], 443);
        assert_eq!(value["ratio"], 0.5);
        let back = super::yaml(&Input {
            mode: 2,
            ..Input::new(json)
        })
        .unwrap();
        assert!(back.contains("name: app"), "{back}");
        let two = super::yaml(&Input::new("a: 1\n---\na: 2\n")).unwrap();
        assert!(two.trim_start().starts_with('['));
        assert!(super::yaml(&Input::new("a: [1")).is_err());
    }

    fn with(text: &str, mode: usize) -> Input {
        Input {
            mode,
            ..Input::new(text)
        }
    }

    #[test]
    fn formats_and_converts() {
        assert_eq!(
            json(&with(r#"{"a": [1, 2]}"#, 1)).unwrap(),
            r#"{"a":[1,2]}"#
        );
        assert!(
            json(&with("{", 0))
                .unwrap_err()
                .to_string()
                .contains("line 1")
        );
        assert_eq!(
            json(&with(r#"{"name": "x"}"#, 2)).unwrap().trim(),
            r#"name = "x""#
        );
        assert!(toml(&with("a = 1", 0)).unwrap().contains(r#""a": 1"#));
        assert_eq!(
            xml(&with("<a><b>x</b></a>", 0)).unwrap(),
            "<a>\n  <b>x</b>\n</a>"
        );
        assert!(xml(&with("<a><b></a>", 0)).is_err());
    }

    #[test]
    fn reads_csv_with_quotes() {
        let text = "name,note\nAda,\"likes, commas\"\nBob,\"said \"\"hi\"\"\"\n";
        let out = csv(&with(text, 0)).unwrap();
        assert!(out.contains(r#""note": "likes, commas""#) && out.contains(r#"said \"hi\""#));
        let table = csv(&with("a\tb\n1\t22\n", 1)).unwrap();
        assert_eq!(table, "a | b\n--+---\n1 | 22");
    }
}
