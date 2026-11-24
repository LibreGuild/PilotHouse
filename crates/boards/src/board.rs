//! A board's file: `site/board.json`, the list of what is on the board. It
//! is the truth: the page draws it, the user's changes are written to it,
//! and an agent changes the board by editing it.

use std::path::Path;

use serde_json::{Value, json};

/// What artifacts that are boards say they are (`artifact.json`).
pub const KIND: &str = "whiteboard";
/// The board's file, in the artifact's `site/`.
pub const BOARD: &str = "board.json";
/// Where pictures put on a board are kept, in `site/`.
pub const MEDIA: &str = "media";
/// A board's file, at most: a page cannot fill the disk with one.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// The things an outline lists, at most.
const MAX_LISTED: usize = 400;

/// A board with nothing on it.
pub fn empty(title: &str) -> String {
    let board = json!({ "version": 1, "title": title, "elements": [] });
    serde_json::to_string_pretty(&board).unwrap_or_else(|_| "{}".into()) + "\n"
}

/// The board in `site` as it reads now: an empty one where there is none
/// yet; an error where there is a file that cannot be read as a board's
/// (too large, not text, not allowed), which is then not taken for an
/// empty board and written over.
pub fn load(site: &Path) -> Result<String, String> {
    let file = site.join(BOARD);
    match std::fs::metadata(&file) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(empty("")),
        Err(err) => return Err(format!("board.json cannot be read: {err}")),
        // Twice what a page may save: an agent's indenting is roomier.
        Ok(file) if file.len() > 2 * MAX_BYTES as u64 => {
            return Err("board.json is too large to be a board".into());
        }
        Ok(_) => {}
    }
    std::fs::read_to_string(&file).map_err(|err| format!("board.json cannot be read: {err}"))
}

/// [`load`], where only what the board reads as matters (its mark, its
/// outline): a file that cannot be read reads as nothing.
pub fn read(site: &Path) -> String {
    load(site).unwrap_or_default()
}

/// A mark of `text`: the same text has the same mark, so a page can say
/// which state of the board its change was made on.
pub fn mark(text: &str) -> String {
    // FNV-1a, 64 bits: a change is told from no change, nothing is guarded.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Whether `text` is a board: JSON with a list of elements. What a page
/// (or an agent) wrote that is not is refused, or said to be broken.
pub fn check(text: &str) -> Result<Value, String> {
    let board: Value =
        serde_json::from_str(text).map_err(|err| format!("board.json is not JSON: {err}"))?;
    if !board["elements"].is_array() {
        return Err("board.json has no list of `elements`".into());
    }
    Ok(board)
}

/// The board in words, for an agent: how many of each kind of thing there
/// are; each section with what lies in it, then what lies loose (each
/// thing with its id, place and what it says); what is joined to what; the
/// notes pinned to the board; and what in the file cannot be drawn as
/// written (an arrow to a thing that is not there, an id used twice).
pub fn outline(text: &str) -> String {
    let board = match check(text) {
        Ok(board) => board,
        Err(why) => return format!("The board cannot be read: {why}. Write it again whole."),
    };
    let elements = board["elements"].as_array().cloned().unwrap_or_default();
    if elements.is_empty() {
        return "The board is empty.".into();
    }
    let word = |element: &Value, key: &str| element[key].as_str().unwrap_or_default().to_string();
    let number = |element: &Value, key: &str| element[key].as_f64().unwrap_or(0.).round() as i64;
    let mut counts: Vec<(String, usize)> = Vec::new();
    for element in &elements {
        let kind = word(element, "type");
        match counts.iter_mut().find(|(known, _)| *known == kind) {
            Some((_, count)) => *count += 1,
            None => counts.push((kind, 1)),
        }
    }
    let mut out = format!(
        "{} things on the board: {}.\n",
        elements.len(),
        counts
            .iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let one_line = |text: String| {
        let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
        match line.char_indices().nth(120) {
            Some((at, _)) => format!("{}…", &line[..at]),
            None => line,
        }
    };
    let quoted = |said: String| match one_line(said) {
        said if said.is_empty() => String::new(),
        said => format!(" “{said}”"),
    };
    // Where a thing is, and how large when the file says.
    let placed = |element: &Value| {
        let at = format!(" at ({}, {})", number(element, "x"), number(element, "y"));
        match (number(element, "w"), number(element, "h")) {
            (w, h) if w > 0 && h > 0 => format!("{at} {w}×{h}"),
            _ => at,
        }
    };
    // A thing that takes room, in a line.
    let thing = |element: &Value| {
        let kind = word(element, "type");
        let said = match kind.as_str() {
            "frame" => word(element, "title"),
            "sticker" => word(element, "emoji"),
            "table" => element["rows"]
                .as_array()
                .and_then(|rows| rows.first())
                .and_then(Value::as_array)
                .map(|cells| {
                    cells
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
                .unwrap_or_default(),
            _ => word(element, "text"),
        };
        let detail = match kind.as_str() {
            "draw" => format!(
                " ({} points)",
                element["points"].as_array().map_or(0, Vec::len)
            ),
            "shape" => format!(" {}{}", word(element, "shape"), placed(element)),
            _ => placed(element),
        };
        let fixed = if element["locked"].as_bool() == Some(true) {
            " (locked)"
        } else {
            ""
        };
        format!(
            "- {} {kind}{detail}{}{fixed}\n",
            word(element, "id"),
            quoted(said)
        )
    };
    let kind_is = |element: &Value, kind: &str| element["type"].as_str() == Some(kind);
    // A thing lies in the section its middle is in (the last such one, as
    // the page has it: the one drawn on top).
    let inside = |element: &Value, frame: &Value| {
        let middle = |at: &str, size: &str| {
            element[at].as_f64().unwrap_or(0.) + element[size].as_f64().unwrap_or(0.) / 2.
        };
        let within = |value: f64, at: &str, size: &str| {
            let from = frame[at].as_f64().unwrap_or(0.);
            (from..=from + frame[size].as_f64().unwrap_or(0.)).contains(&value)
        };
        within(middle("x", "w"), "x", "w") && within(middle("y", "h"), "y", "h")
    };
    let mut listed = String::new();
    let frames: Vec<&Value> = elements
        .iter()
        .filter(|element| kind_is(element, "frame"))
        .collect();
    // (A stroke of the pen is counted above, not listed: a drawing is
    // hundreds of them.)
    let roomy = |element: &Value| {
        !["line", "comment", "frame", "draw"]
            .iter()
            .any(|kind| kind_is(element, kind))
    };
    let section_of = |element: &Value| frames.iter().rposition(|frame| inside(element, frame));
    for (place, frame) in frames.iter().enumerate() {
        listed.push_str(&thing(frame));
        for element in elements
            .iter()
            .filter(|element| roomy(element) && section_of(element) == Some(place))
        {
            listed.push_str("  ");
            listed.push_str(&thing(element));
        }
    }
    let loose: Vec<&Value> = elements
        .iter()
        .filter(|element| roomy(element) && section_of(element).is_none())
        .collect();
    if !loose.is_empty() && !frames.is_empty() {
        listed.push_str("In no section:\n");
    }
    for element in loose {
        listed.push_str(&thing(element));
    }
    let known = |id: &str| {
        elements
            .iter()
            .any(|element| element["id"].as_str() == Some(id))
    };
    let mut wrong: Vec<String> = Vec::new();
    for line in elements.iter().filter(|element| kind_is(element, "line")) {
        let end = |key: &str| match line[key]["id"].as_str() {
            Some(id) => id.to_string(),
            None => format!("({}, {})", number(&line[key], "x"), number(&line[key], "y")),
        };
        listed.push_str(&format!(
            "- {} line from {} to {}{}\n",
            word(line, "id"),
            end("from"),
            end("to"),
            quoted(word(line, "text"))
        ));
        for key in ["from", "to"] {
            if let Some(id) = line[key]["id"].as_str()
                && !known(id)
            {
                wrong.push(format!(
                    "line {}: its `{key}` names `{id}`, which is not on the board (it is drawn from the board's corner)",
                    word(line, "id")
                ));
            }
        }
    }
    for note in elements
        .iter()
        .filter(|element| kind_is(element, "comment"))
    {
        let replies = note["replies"].as_array().map_or(0, Vec::len);
        listed.push_str(&format!(
            "- {} comment{}{} ({}{})\n",
            word(note, "id"),
            placed(note),
            quoted(word(note, "text")),
            if note["resolved"].as_bool() == Some(true) {
                "resolved"
            } else {
                "open"
            },
            match replies {
                0 => String::new(),
                1 => ", 1 reply".into(),
                n => format!(", {n} replies"),
            }
        ));
    }
    // A board of thousands of things is not read whole in one answer.
    let lines: Vec<&str> = listed.lines().collect();
    for line in lines.iter().take(MAX_LISTED) {
        out.push_str(line);
        out.push('\n');
    }
    if lines.len() > MAX_LISTED {
        out.push_str(&format!(
            "… and {} more (board.json has them all)\n",
            lines.len() - MAX_LISTED
        ));
    }
    let mut seen: Vec<&str> = Vec::new();
    for element in &elements {
        match element["id"].as_str().filter(|id| !id.is_empty()) {
            None => wrong.push(format!(
                "a {} has no `id`: give it one",
                match word(element, "type") {
                    kind if kind.is_empty() => "thing".to_string(),
                    kind => kind,
                }
            )),
            Some(id) if seen.contains(&id) => {
                wrong.push(format!("two things have the id `{id}`: ids are unique"))
            }
            Some(id) => seen.push(id),
        }
    }
    if !wrong.is_empty() {
        out.push_str("To fix in board.json:\n");
        for line in wrong {
            out.push_str(&format!("- {line}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_is_checked_marked_and_said_in_words() {
        let empty = empty("Plan");
        assert!(check(&empty).is_ok());
        assert_eq!(outline(&empty), "The board is empty.");
        assert_eq!(mark(&empty), mark(&empty.clone()));
        assert_ne!(mark(&empty), mark("{}"));
        assert!(check("{}").is_err() && check("not json").is_err());
        assert!(outline("nope").starts_with("The board cannot be read"));

        let board = json!({ "version": 1, "elements": [
            { "id": "a", "type": "sticky", "x": 10, "y": 20, "w": 200, "h": 200, "text": "Ship   it\nsoon" },
            { "id": "b", "type": "shape", "shape": "diamond", "x": 700, "y": 0, "w": 160, "h": 100, "text": "OK?" },
            { "id": "c", "type": "line", "from": { "id": "a" }, "to": { "x": 5, "y": 6 } },
            { "id": "f", "type": "frame", "x": 0, "y": 0, "w": 600, "h": 400, "title": "Now" }
        ] })
        .to_string();
        let said = outline(&board);
        assert!(
            said.starts_with("4 things on the board: 1 sticky, 1 shape, 1 line, 1 frame."),
            "{said}"
        );
        assert!(
            said.contains("- a sticky at (10, 20) 200×200 “Ship it soon”"),
            "{said}"
        );
        assert!(
            said.contains("- b shape diamond at (700, 0) 160×100 “OK?”"),
            "{said}"
        );
        assert!(said.contains("- c line from a to (5, 6)"), "{said}");
        assert!(said.contains("- f frame at (0, 0) 600×400 “Now”"), "{said}");
        // What lies in the section is under it; the rest is said to be loose.
        let (section, loose) = said.split_once("In no section:").expect(&said);
        assert!(
            section.contains("  - a sticky") && loose.contains("- b shape"),
            "{said}"
        );
        assert!(!said.contains("To fix"), "{said}");
    }

    #[test]
    fn what_cannot_be_drawn_as_written_is_said() {
        let board = json!({ "version": 1, "elements": [
            { "id": "a", "type": "sticky", "x": 0, "y": 0, "text": "One" },
            { "id": "a", "type": "sticky", "x": 300, "y": 0, "text": "Two", "locked": true },
            { "type": "text", "x": 0, "y": 300, "text": "No id" },
            { "id": "l", "type": "line", "from": { "id": "a" }, "to": { "id": "gone" }, "text": "then" },
            { "id": "n", "type": "comment", "x": 5, "y": 5, "text": "Why?", "replies": [{ "by": "Agent", "text": "Because" }] }
        ] })
        .to_string();
        let said = outline(&board);
        // No size in the file: none is made up.
        assert!(said.contains("- a sticky at (0, 0) “One”"), "{said}");
        assert!(said.contains("“Two” (locked)"), "{said}");
        assert!(said.contains("- l line from a to gone “then”"), "{said}");
        assert!(
            said.contains("- n comment at (5, 5) “Why?” (open, 1 reply)"),
            "{said}"
        );
        let fix = said.split_once("To fix in board.json:").expect(&said).1;
        assert!(fix.contains("line l: its `to` names `gone`"), "{fix}");
        assert!(fix.contains("two things have the id `a`"), "{fix}");
        assert!(fix.contains("a text has no `id`"), "{fix}");
    }
}
