//! Data formats: JSON, TOML, YAML, XML and CSV.
//!
//! [`convert`] is what the Utilities dialog draws: the result and how big
//! the data is, or a [`Problem`] that says where the text went wrong. The
//! text functions below it are what agents run (`util_run`).

use anyhow::{Result, bail};
use serde_json::Value;

use crate::Input;

/// A data format the dialog reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Yaml,
    Toml,
    Xml,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Json => "JSON",
            Format::Yaml => "YAML",
            Format::Toml => "TOML",
            Format::Xml => "XML",
        }
    }

    /// The name the code editor colours it by.
    pub fn language(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Yaml => "yaml",
            Format::Toml => "toml",
            Format::Xml => "xml",
        }
    }
}

/// What a conversion makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The same format, laid out to be read.
    Pretty,
    /// The same format on one line.
    Minified,
    /// Another format.
    To(Format),
    /// CSV, from a list of objects.
    Csv,
}

/// How the result is laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Layout {
    pub indent: Indent,
    /// Keys in alphabetical order, at every level.
    pub sort_keys: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Indent {
    #[default]
    Two,
    Four,
    Tab,
}

impl Indent {
    fn text(self) -> &'static [u8] {
        match self {
            Indent::Two => b"  ",
            Indent::Four => b"    ",
            Indent::Tab => b"\t",
        }
    }
}

/// Why a text could not be read, and where (both from 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl Problem {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
            column: None,
        }
    }

    fn at(message: impl Into<String>, line: usize, column: usize) -> Self {
        Self {
            message: message.into(),
            line: Some(line),
            column: Some(column),
        }
    }

    /// At byte `offset` of `text`.
    fn at_offset(message: impl Into<String>, text: &str, offset: usize) -> Self {
        let before = &text[..offset.min(text.len())];
        let line = before.matches('\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .map_or(0, |rest| rest.chars().count())
            + 1;
        Self::at(message, line, column)
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => {
                write!(f, "{} (line {line}, column {column})", self.message)
            }
            _ => f.write_str(&self.message),
        }
    }
}

impl From<anyhow::Error> for Problem {
    fn from(err: anyhow::Error) -> Self {
        Problem::new(format!("{err:#}"))
    }
}

/// A conversion's result.
#[derive(Debug, Clone, PartialEq)]
pub struct Converted {
    pub text: String,
    /// Something the reader did on the way, said once (JSON that was
    /// inside a string, several YAML documents).
    pub note: Option<&'static str>,
    /// Values in the data (objects, lists and what they hold), and how deep
    /// it nests; none for XML.
    pub values: Option<usize>,
    pub depth: Option<usize>,
}

/// Reads `text` as `from` and makes `target` of it.
pub fn convert(
    from: Format,
    target: Target,
    text: &str,
    layout: Layout,
) -> Result<Converted, Problem> {
    if from == Format::Xml {
        return xml_layout(text, target == Target::Minified).map(|(text, note)| Converted {
            text,
            note,
            values: None,
            depth: None,
        });
    }
    // YAML laid out again keeps its documents apart.
    if from == Format::Yaml && target == Target::Pretty {
        let docs = yaml_documents(text)?;
        let parts: Result<Vec<String>> = docs.iter().map(emit_yaml).collect();
        let values: Vec<Value> = docs.iter().map(json_of_yaml).collect::<Result<_>>()?;
        let all = Value::Array(values);
        return Ok(Converted {
            text: parts?.join("\n---\n"),
            note: (docs.len() > 1).then_some("Several documents (---)."),
            values: Some(count(&all) - 1),
            depth: Some(depth(&all).saturating_sub(1)),
        });
    }
    let (mut value, mut note) = read_value(from, text)?;
    if from == Format::Yaml && value.is_array() && text.contains("\n---") {
        note = note.or(Some("Several documents (---) became one list."));
    }
    if layout.sort_keys {
        value = sorted(value);
    }
    let out = match (target, from) {
        (Target::Pretty, Format::Json) | (Target::To(Format::Json), _) => {
            pretty_json(&value, layout.indent)?
        }
        (Target::Minified, _) => serde_json::to_string(&value).map_err(anyhow::Error::from)?,
        (Target::Pretty, Format::Toml) | (Target::To(Format::Toml), _) => toml_of(&value)?,
        (Target::Pretty, _) | (Target::To(Format::Yaml), _) => emit_yaml(&yaml_of(&value))?,
        (Target::To(Format::Xml), _) => {
            return Err(Problem::new("XML cannot be made from other formats here."));
        }
        (Target::Csv, _) => csv_of(&value)?,
    };
    Ok(Converted {
        text: out,
        note,
        values: Some(count(&value)),
        depth: Some(depth(&value)),
    })
}

/// The value `text` holds, read as `format`, and a note when it was found
/// in an unusual place (JSON inside a string, escaped JSON).
fn read_value(format: Format, text: &str) -> Result<(Value, Option<&'static str>), Problem> {
    match format {
        Format::Json => read_json(text),
        Format::Toml => {
            let table: ::toml::Table = text.parse().map_err(|err: ::toml::de::Error| {
                let message = format!("Not valid TOML: {}", err.message().trim());
                match err.span() {
                    Some(span) => Problem::at_offset(message, text, span.start),
                    None => Problem::new(message),
                }
            })?;
            let value = serde_json::to_value(table).map_err(anyhow::Error::from)?;
            Ok((value, None))
        }
        Format::Yaml => {
            let docs = yaml_documents(text)?;
            let value = match docs.as_slice() {
                [] => Value::Null,
                [one] => json_of_yaml(one)?,
                many => Value::Array(many.iter().map(json_of_yaml).collect::<Result<_>>()?),
            };
            Ok((value, None))
        }
        Format::Xml => Err(Problem::new("XML is not read as data here.")),
    }
}

/// JSON, or JSON that was put in a string (a log's `"{\"a\": 1}"`), or
/// escaped without its quotes (`{\"a\": 1}`): the data it holds.
fn read_json(text: &str) -> Result<(Value, Option<&'static str>), Problem> {
    if text.trim().is_empty() {
        return Err(Problem::new("There is nothing to read."));
    }
    let inner = |text: &str| -> Option<Value> {
        let trimmed = text.trim_start();
        if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
            return None;
        }
        serde_json::from_str::<Value>(text).ok()
    };
    match serde_json::from_str::<Value>(text) {
        Ok(Value::String(held)) => match inner(&held) {
            Some(value) => Ok((
                value,
                Some("This was JSON inside a string; this is what it holds."),
            )),
            None => Ok((Value::String(held), None)),
        },
        Ok(value) => Ok((value, None)),
        Err(err) => {
            if text.contains("\\\"") {
                let quoted = format!("\"{}\"", text.trim());
                if let Ok(held) = serde_json::from_str::<String>(&quoted)
                    && let Some(value) = inner(&held)
                {
                    return Ok((
                        value,
                        Some("This was escaped JSON (\\\"); this is what it holds."),
                    ));
                }
            }
            let message = format!("Not valid JSON: {}", plain_serde_error(&err));
            Err(if err.line() > 0 {
                Problem::at(message, err.line(), err.column().max(1))
            } else {
                Problem::new(message)
            })
        }
    }
}

/// serde's message without its own " at line 3 column 7", which the
/// problem says on its own.
fn plain_serde_error(err: &serde_json::Error) -> String {
    let text = err.to_string();
    match text.rfind(" at line ") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

fn yaml_documents(text: &str) -> Result<Vec<yaml_rust2::Yaml>, Problem> {
    within_yaml_limits(text).map_err(Problem::from)?;
    yaml_rust2::YamlLoader::load_from_str(text).map_err(|err| {
        let marker = err.marker();
        // The reader counts columns from 0.
        Problem::at(
            format!("Not valid YAML: {}", err.info()),
            marker.line(),
            marker.col() + 1,
        )
    })
}

fn pretty_json(value: &Value, indent: Indent) -> Result<String, Problem> {
    use serde::Serialize as _;
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.text());
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    value
        .serialize(&mut serializer)
        .map_err(anyhow::Error::from)?;
    Ok(String::from_utf8(out).map_err(anyhow::Error::from)?)
}

fn toml_of(value: &Value) -> Result<String, Problem> {
    if !value.is_object() {
        return Err(Problem::new("TOML needs an object at the top level."));
    }
    ::toml::to_string_pretty(value)
        .map_err(|_| Problem::new("TOML cannot hold this (null values, or lists of mixed kinds)."))
}

/// Every key in alphabetical order, at every level.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, sorted(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}

/// Values in `value`, itself included.
fn count(value: &Value) -> usize {
    1 + match value {
        Value::Object(map) => map.values().map(count).sum(),
        Value::Array(items) => items.iter().map(count).sum(),
        _ => 0,
    }
}

/// Levels of objects and lists; a plain value has none.
fn depth(value: &Value) -> usize {
    match value {
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// A list of objects as CSV: a column for each key, in the order first
/// seen; nested values as JSON.
fn csv_of(value: &Value) -> Result<String, Problem> {
    let Some(items) = value.as_array() else {
        return Err(Problem::new("CSV needs a list of objects ([{…}, {…}])."));
    };
    let mut columns: Vec<String> = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            return Err(Problem::new(
                "CSV needs a list of objects; this list holds other values.",
            ));
        };
        for key in object.keys() {
            if !columns.contains(key) {
                columns.push(key.clone());
            }
        }
    }
    let cell = |value: Option<&Value>| -> String {
        let text = match value {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
        };
        csv_cell(&text)
    };
    let mut lines = vec![
        columns
            .iter()
            .map(|column| csv_cell(column))
            .collect::<Vec<_>>()
            .join(","),
    ];
    for item in items {
        lines.push(
            columns
                .iter()
                .map(|column| cell(item.get(column)))
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    Ok(lines.join("\n"))
}

/// A cell quoted when it holds a comma, a quote or a line break.
fn csv_cell(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// Format, minify, to TOML, to YAML, or to CSV.
pub fn json(input: &Input) -> Result<String> {
    let target = match input.mode {
        0 => Target::Pretty,
        1 => Target::Minified,
        2 => Target::To(Format::Toml),
        3 => Target::To(Format::Yaml),
        _ => Target::Csv,
    };
    text_of(convert(
        Format::Json,
        target,
        &input.text,
        Layout::default(),
    ))
}

/// To JSON, formatted, or to YAML.
pub fn toml(input: &Input) -> Result<String> {
    let target = match input.mode {
        0 => Target::To(Format::Json),
        1 => Target::Pretty,
        _ => Target::To(Format::Yaml),
    };
    text_of(convert(
        Format::Toml,
        target,
        &input.text,
        Layout::default(),
    ))
}

/// To JSON, formatted, JSON to YAML, or to TOML. Several documents (`---`)
/// become a JSON array.
pub fn yaml(input: &Input) -> Result<String> {
    let converted = match input.mode {
        0 => convert(
            Format::Yaml,
            Target::To(Format::Json),
            &input.text,
            Layout::default(),
        ),
        1 => convert(Format::Yaml, Target::Pretty, &input.text, Layout::default()),
        2 => convert(
            Format::Json,
            Target::To(Format::Yaml),
            &input.text,
            Layout::default(),
        ),
        _ => convert(
            Format::Yaml,
            Target::To(Format::Toml),
            &input.text,
            Layout::default(),
        ),
    };
    text_of(converted)
}

fn text_of(converted: Result<Converted, Problem>) -> Result<String> {
    converted
        .map(|converted| converted.text)
        .map_err(|problem| anyhow::anyhow!("{problem}"))
}

fn json_of_yaml(yaml: &yaml_rust2::Yaml) -> Result<Value> {
    use yaml_rust2::Yaml;
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
        Yaml::Array(items) => Value::Array(items.iter().map(json_of_yaml).collect::<Result<_>>()?),
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
                object.insert(key, json_of_yaml(value)?);
            }
            Value::Object(object)
        }
        Yaml::Alias(_) => bail!("YAML aliases (*name) are not supported."),
    })
}

/// How deep a YAML text may nest, and how much it may stand for once its
/// aliases are copied out. It is read inside the app, also for agents: a
/// few lines can nest deeper than the reader's stack, or through aliases
/// (each `*name` is a copy of its anchor's whole node) stand for billions
/// of nodes.
const YAML_DEPTH: usize = 128;
const YAML_NODES: usize = 1_000_000;
const YAML_BYTES: usize = 64 << 20;

/// Refuses a text over the limits above before it is loaded. The parser's
/// events are walked one by one (it keeps its own stack, so this cannot
/// overflow), counting each alias as what it copies.
fn within_yaml_limits(text: &str) -> Result<()> {
    use yaml_rust2::parser::{Event, Parser};

    /// What a node stands for: nodes, bytes of text, and the levels of
    /// lists and maps it holds.
    #[derive(Clone, Copy)]
    struct Size {
        nodes: usize,
        bytes: usize,
        height: usize,
    }
    /// A list or map still open.
    struct Open {
        anchor: usize,
        nodes: usize,
        bytes: usize,
        level: usize,
        deepest: usize,
    }

    let too_big = || {
        anyhow::anyhow!(
            "This YAML stands for more than {YAML_NODES} values or {} MB once its aliases (*name) are copied out.",
            YAML_BYTES >> 20
        )
    };
    let too_deep = || anyhow::anyhow!("This YAML nests more than {YAML_DEPTH} levels deep.");
    let mut parser = Parser::new_from_str(text);
    let mut anchors: std::collections::HashMap<usize, Size> = std::collections::HashMap::new();
    let mut open: Vec<Open> = Vec::new();
    let (mut nodes, mut bytes) = (0usize, 0usize);
    loop {
        // A text that is not YAML is the loader's to report, where it went
        // wrong: everything before that was within the limits.
        let Ok((event, _)) = parser.next_token() else {
            return Ok(());
        };
        match event {
            Event::StreamEnd => return Ok(()),
            Event::Scalar(value, _, anchor, _) => {
                nodes += 1;
                bytes += value.len();
                if anchor != 0 {
                    let size = Size {
                        nodes: 1,
                        bytes: value.len(),
                        height: 0,
                    };
                    anchors.insert(anchor, size);
                }
            }
            Event::Alias(anchor) => {
                // An alias to no anchor is the loader's error to report.
                let Some(size) = anchors.get(&anchor).copied() else {
                    continue;
                };
                nodes = nodes.saturating_add(size.nodes);
                bytes = bytes.saturating_add(size.bytes);
                let reached = open.len() + size.height;
                if reached > YAML_DEPTH {
                    return Err(too_deep());
                }
                if let Some(parent) = open.last_mut() {
                    parent.deepest = parent.deepest.max(reached);
                }
            }
            Event::SequenceStart(anchor, _) | Event::MappingStart(anchor, _) => {
                let level = open.len() + 1;
                if level > YAML_DEPTH {
                    return Err(too_deep());
                }
                open.push(Open {
                    anchor,
                    nodes,
                    bytes,
                    level,
                    deepest: level,
                });
                nodes += 1;
            }
            Event::SequenceEnd | Event::MappingEnd => {
                if let Some(done) = open.pop() {
                    if done.anchor != 0 {
                        let size = Size {
                            nodes: nodes - done.nodes,
                            bytes: bytes - done.bytes,
                            height: done.deepest + 1 - done.level,
                        };
                        anchors.insert(done.anchor, size);
                    }
                    if let Some(parent) = open.last_mut() {
                        parent.deepest = parent.deepest.max(done.deepest);
                    }
                }
            }
            _ => {}
        }
        if nodes > YAML_NODES || bytes > YAML_BYTES {
            return Err(too_big());
        }
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
    xml_layout(&input.text, input.mode != 0)
        .map(|(text, _)| text)
        .map_err(|problem| anyhow::anyhow!("{problem}"))
}

/// The XML laid out, and a note when it uses a prefix it does not declare
/// (an element copied out of an Android layout: `android:` is declared at
/// the top of the file, not in the part copied), which is laid out all the
/// same.
fn xml_layout(text: &str, minified: bool) -> Result<(String, Option<&'static str>), Problem> {
    let text = text.trim();
    let note = match roxmltree::Document::parse(text) {
        Ok(_) => None,
        Err(roxmltree::Error::UnknownNamespace(..)) => Some(
            "A prefix (such as android:) is used without its xmlns declaration: fine in a snippet.",
        ),
        Err(err) => {
            let at = err.pos();
            return Err(Problem::at(
                format!("Not valid XML: {err}"),
                at.row as usize,
                at.col as usize,
            ));
        }
    };
    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().trim_text(true);
    let mut writer = if minified {
        quick_xml::Writer::new(Vec::new())
    } else {
        quick_xml::Writer::new_with_indent(Vec::new(), b' ', 2)
    };
    loop {
        match reader.read_event().map_err(anyhow::Error::from)? {
            quick_xml::events::Event::Eof => break,
            event => writer.write_event(event).map_err(anyhow::Error::from)?,
        }
    }
    Ok((
        String::from_utf8(writer.into_inner()).map_err(anyhow::Error::from)?,
        note,
    ))
}

/// Rows read from CSV: the header (when the first row is one) and the
/// rest, and the character that separated the cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub separator: char,
}

impl Table {
    pub fn columns(&self) -> usize {
        self.rows
            .iter()
            .map(Vec::len)
            .chain([self.header.len()])
            .max()
            .unwrap_or(0)
    }
}

/// CSV, TSV, or cells separated by `;` or `|` (the one the first line has
/// most of, outside quotes); with `header`, the first row names the
/// columns, otherwise they are numbered.
pub fn table(text: &str, header: bool) -> Result<Table> {
    let separator = separator_of(text);
    let mut rows = parse_csv(text, separator);
    if rows.is_empty() {
        bail!("There are no rows.");
    }
    let header = if header {
        rows.remove(0)
    } else {
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        (1..=columns).map(|n| format!("Column {n}")).collect()
    };
    Ok(Table {
        header,
        rows,
        separator,
    })
}

/// CSV to JSON objects by the header, an aligned table, or JSON to CSV.
pub fn csv(input: &Input) -> Result<String> {
    if input.mode == 2 {
        return text_of(convert(
            Format::Json,
            Target::Csv,
            &input.text,
            Layout::default(),
        ));
    }
    let Table { header, rows, .. } = table(&input.text, true)?;
    if input.mode == 0 {
        return Ok(serde_json::to_string_pretty(&objects(&header, &rows))?);
    }
    let all: Vec<&Vec<String>> = std::iter::once(&header).chain(&rows).collect();
    let columns = all.iter().map(|row| row.len()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            all.iter()
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
    let mut out = vec![line(&header)];
    out.push(
        widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<_>>()
            .join("-+-"),
    );
    out.extend(rows.iter().map(line));
    Ok(out.join("\n"))
}

/// The rows as JSON objects keyed by the header.
pub fn objects(header: &[String], rows: &[Vec<String>]) -> Value {
    Value::Array(
        rows.iter()
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
            .collect(),
    )
}

/// The separator the first line uses most, outside quotes: `,`, tab, `;`
/// (spreadsheets where the comma is the decimal mark) or `|`.
fn separator_of(text: &str) -> char {
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let mut counts = [(',', 0usize), ('\t', 0), (';', 0), ('|', 0)];
    let mut quoted = false;
    for c in first.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && let Some(slot) = counts.iter_mut().find(|(sep, _)| *sep == c) {
            slot.1 += 1;
        }
    }
    counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .filter(|(_, count)| *count > 0)
        .map_or(',', |(sep, _)| *sep)
}

/// Rows of cells; quotes may hold the separator, newlines and `""`.
fn parse_csv(text: &str, separator: char) -> Vec<Vec<String>> {
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

    #[test]
    fn yaml_that_would_grow_without_end_is_refused() {
        // Each line ten copies of the one before: a billion values in nine.
        let mut laughs = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
        for level in 1..9 {
            let copies = vec![format!("*a{}", level - 1); 10].join(", ");
            laughs.push_str(&format!("a{level}: &a{level} [{copies}]\n"));
        }
        let err = super::yaml(&Input::new(laughs)).unwrap_err().to_string();
        assert!(err.contains("aliases"), "{err}");
        // Deeper than any stack, in one line.
        let deep = "- ".repeat(20_000) + "x";
        let err = super::yaml(&Input::new(deep)).unwrap_err().to_string();
        assert!(err.contains("levels deep"), "{err}");
        // An alias that reaches deeper than its own line.
        let nested = format!("a: &a {}x{}\nb: [[*a]]\n", "[".repeat(127), "]".repeat(127));
        assert!(super::yaml(&Input::new(nested)).is_err());
        // Aliases within the limits still load.
        let json = super::yaml(&Input::new("a: &a [1, 2]\nb: *a\n")).unwrap();
        assert!(json.contains("\"b\": [\n    1,"), "{json}");
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
        // Semicolons, as a spreadsheet with a decimal comma writes them.
        let read = super::table("a;b\n1,5;2\n", true).unwrap();
        assert_eq!((read.separator, read.rows[0][0].as_str()), (';', "1,5"));
        let numbered = super::table("x,y\n", false).unwrap();
        assert_eq!(numbered.header, ["Column 1", "Column 2"]);
        // And back: a list of objects to CSV, quoting what needs it.
        let back = csv(&with(r#"[{"a": 1, "b": "x,y"}, {"c": null}]"#, 2)).unwrap();
        assert_eq!(back, "a,b,c\n1,\"x,y\",\n,,");
    }

    #[test]
    fn says_where_a_text_goes_wrong() {
        let problem = convert(
            Format::Json,
            Target::Pretty,
            "{\n  \"a\": 1,\n  oops\n}",
            Layout::default(),
        )
        .unwrap_err();
        assert_eq!((problem.line, problem.column), (Some(3), Some(3)));
        assert!(!problem.message.contains("at line"), "{}", problem.message);
        let problem = convert(
            Format::Toml,
            Target::Pretty,
            "a = 1\nb = \n",
            Layout::default(),
        )
        .unwrap_err();
        assert_eq!(problem.line, Some(2));
        let problem =
            convert(Format::Yaml, Target::Pretty, "a: [1\n", Layout::default()).unwrap_err();
        assert!(problem.line.is_some());
        let problem = convert(
            Format::Xml,
            Target::Pretty,
            "<a>\n<b></a>",
            Layout::default(),
        )
        .unwrap_err();
        assert_eq!(problem.line, Some(2));
        let problem = convert(
            Format::Yaml,
            Target::Pretty,
            "a: 1\n b: 2\n",
            Layout::default(),
        )
        .unwrap_err();
        assert!(problem.column.is_some_and(|column| column >= 1));
        // A snippet of a layout, its prefix declared elsewhere.
        let snippet = convert(
            Format::Xml,
            Target::Pretty,
            r#"<TextView android:text="Hi"/>"#,
            Layout::default(),
        )
        .unwrap();
        assert!(snippet.note.is_some() && snippet.text.contains("android:text"));
    }

    #[test]
    fn reads_json_kept_in_a_string_and_lays_it_out() {
        let held = convert(
            Format::Json,
            Target::Minified,
            r#""{\"a\": [1, 2]}""#,
            Layout::default(),
        )
        .unwrap();
        assert_eq!(held.text, r#"{"a":[1,2]}"#);
        assert!(held.note.is_some());
        let escaped = convert(
            Format::Json,
            Target::Minified,
            r#"{\"a\": 1}"#,
            Layout::default(),
        )
        .unwrap();
        assert_eq!(escaped.text, r#"{"a":1}"#);
        let sorted = convert(
            Format::Json,
            Target::Pretty,
            r#"{"b": {"z": 1, "y": 2}, "a": 0}"#,
            Layout {
                indent: Indent::Four,
                sort_keys: true,
            },
        )
        .unwrap();
        assert_eq!(
            sorted.text,
            "{\n    \"a\": 0,\n    \"b\": {\n        \"y\": 2,\n        \"z\": 1\n    }\n}"
        );
        assert_eq!((sorted.values, sorted.depth), (Some(5), Some(2)));
        // Keys keep their order unless asked.
        let kept = convert(
            Format::Json,
            Target::Minified,
            r#"{"b": 1, "a": 2}"#,
            Layout::default(),
        )
        .unwrap();
        assert_eq!(kept.text, r#"{"b":1,"a":2}"#);
        let yaml = convert(
            Format::Toml,
            Target::To(Format::Yaml),
            "a = 1\n[b]\nc = \"x\"\n",
            Layout::default(),
        )
        .unwrap();
        assert!(yaml.text.contains("c: x"), "{}", yaml.text);
    }
}
