//! The on-screen UI hierarchy from `uiautomator dump`.
//!
//! The XML is flat enough to scan `<node … />` tags directly, so no XML
//! library is needed.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiNode {
    pub class: String,
    pub resource_id: String,
    pub text: String,
    pub description: String,
    pub clickable: bool,
    /// Typing goes here.
    pub focused: bool,
    /// A field that hides what is typed.
    pub password: bool,
    /// Device pixels: left, top, right, bottom.
    pub bounds: (i32, i32, i32, i32),
}

impl UiNode {
    pub fn center(&self) -> (i32, i32) {
        (
            (self.bounds.0 + self.bounds.2) / 2,
            (self.bounds.1 + self.bounds.3) / 2,
        )
    }

    fn area(&self) -> i64 {
        (self.bounds.2 - self.bounds.0) as i64 * (self.bounds.3 - self.bounds.1) as i64
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.bounds.0 && x < self.bounds.2 && y >= self.bounds.1 && y < self.bounds.3
    }

    fn is_interesting(&self) -> bool {
        self.clickable
            || !self.text.is_empty()
            || !self.description.is_empty()
            || !self.resource_id.is_empty()
    }

    /// One line for agents and the picker.
    pub fn describe(&self) -> String {
        let class = self.class.rsplit('.').next().unwrap_or(&self.class);
        let mut line = class.to_string();
        if !self.text.is_empty() {
            line.push_str(&format!(" \"{}\"", self.text));
        }
        if !self.description.is_empty() {
            line.push_str(&format!(" desc=\"{}\"", self.description));
        }
        if !self.resource_id.is_empty() {
            line.push_str(&format!(" id={}", self.resource_id));
        }
        if self.clickable {
            line.push_str(" clickable");
        }
        let (l, t, r, b) = self.bounds;
        let (x, y) = self.center();
        line.push_str(&format!(" bounds=[{l},{t}][{r},{b}] center=({x},{y})"));
        line
    }
}

pub fn parse(xml: &str) -> Vec<UiNode> {
    let mut nodes = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<node ") {
        rest = &rest[start + 6..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        let attr = |name: &str| attribute(tag, name).unwrap_or_default();
        if let Some(bounds) = parse_bounds(&attr("bounds")) {
            nodes.push(UiNode {
                class: attr("class"),
                resource_id: attr("resource-id"),
                text: attr("text"),
                description: attr("content-desc"),
                clickable: attr("clickable") == "true",
                focused: attr("focused") == "true",
                password: attr("password") == "true",
                bounds,
            });
        }
        rest = &rest[end..];
    }
    nodes
}

/// Nodes worth listing: those with text, an id, a description, or a tap target.
pub fn summary(nodes: &[UiNode]) -> String {
    nodes
        .iter()
        .filter(|node| node.is_interesting())
        .map(UiNode::describe)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The smallest node containing a point, preferring interesting ones.
pub fn node_at(nodes: &[UiNode], x: i32, y: i32) -> Option<&UiNode> {
    let containing = || nodes.iter().filter(move |node| node.contains(x, y));
    containing()
        .filter(|node| node.is_interesting())
        .min_by_key(|node| node.area())
        .or_else(|| containing().min_by_key(|node| node.area()))
}

/// What the screen says: each element's text and description, a line each
/// (no ids or class names, which are not words a user sees).
pub fn texts(nodes: &[UiNode]) -> String {
    nodes
        .iter()
        .flat_map(|node| [&node.text, &node.description])
        .filter(|text| !text.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The screen's size in device pixels: the far corner of what is on it.
pub fn screen(nodes: &[UiNode]) -> Option<(i32, i32)> {
    let width = nodes.iter().map(|node| node.bounds.2).max()?;
    let height = nodes.iter().map(|node| node.bounds.3).max()?;
    (width > 0 && height > 0).then_some((width, height))
}

/// An element as a recorded tap names it: by its id, the words on it or its
/// description, whichever it has. Its place on the screen is not part of
/// it: that differs from phone to phone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mark {
    pub id: String,
    pub text: String,
    pub description: String,
}

impl Mark {
    pub fn of(node: &UiNode) -> Self {
        Self {
            id: node.resource_id.clone(),
            text: node.text.clone(),
            description: node.description.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.id.is_empty() && self.text.is_empty() && self.description.is_empty()
    }

    fn says(&self) -> String {
        if !self.text.is_empty() {
            format!("“{}”", self.text)
        } else if !self.description.is_empty() {
            format!("“{}”", self.description)
        } else {
            self.id.clone()
        }
    }
}

/// An id without its app: `com.app.debug:id/save` and `com.app:id/save` are
/// the same element in two builds of one app.
fn bare_id(id: &str) -> &str {
    id.rsplit_once(":id/").map_or(id, |(_, name)| name)
}

/// The node `mark` names. With an id and words: the node that has both;
/// else the only node of that id (its words changed); else, where the id
/// is one many rows share, the node that says the words. Never another row
/// of the same id: a tap there would be a tap on something else. With only
/// an id, or only words, the nodes that have it.
///
/// Of several, the one nearest `near` (where it was tapped when recorded,
/// on this screen's scale).
pub fn find<'a>(
    nodes: &'a [UiNode],
    mark: &Mark,
    near: Option<(i32, i32)>,
) -> Result<&'a UiNode, String> {
    let by = |keep: &dyn Fn(&UiNode) -> bool| -> Vec<&'a UiNode> {
        nodes.iter().filter(|node| keep(node)).collect()
    };
    let has_words = !mark.text.is_empty() || !mark.description.is_empty();
    let says = |node: &UiNode| {
        (!mark.text.is_empty() && node.text.trim() == mark.text.trim())
            || (!mark.description.is_empty() && node.description.trim() == mark.description.trim())
    };
    let of_id = |node: &UiNode| {
        !node.resource_id.is_empty() && bare_id(&node.resource_id) == bare_id(&mark.id)
    };
    let mut found = if mark.id.is_empty() {
        by(&says)
    } else if !has_words {
        by(&of_id)
    } else {
        let both = by(&|node| of_id(node) && says(node));
        let same_id = by(&of_id);
        if !both.is_empty() {
            both
        } else if same_id.len() == 1 {
            same_id
        } else {
            by(&says)
        }
    };
    if let Some((x, y)) = near {
        found.sort_by_key(|node| {
            let (cx, cy) = node.center();
            (cx - x).abs() as i64 + (cy - y).abs() as i64
        });
    }
    found.first().copied().ok_or_else(|| {
        let there: Vec<&str> = nodes
            .iter()
            .flat_map(|node| [&node.text, &node.description])
            .filter(|text| !text.is_empty())
            .map(String::as_str)
            .take(8)
            .collect();
        format!(
            "nothing on the screen is {}{}",
            mark.says(),
            if there.is_empty() {
                String::new()
            } else {
                format!(" (there: {})", there.join(", "))
            }
        )
    })
}

/// A point recorded on a screen of size `from`, on one of size `to`.
pub fn scale(point: (i32, i32), from: (i32, i32), to: (i32, i32)) -> (i32, i32) {
    if from.0 <= 0 || from.1 <= 0 || from == to {
        return point;
    }
    (
        (point.0 as i64 * to.0 as i64 / from.0 as i64) as i32,
        (point.1 as i64 * to.1 as i64 / from.1 as i64) as i32,
    )
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = start + tag[start..].find('"')?;
    Some(unescape(&tag[start..end]))
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#10;", "\n")
        .replace("&amp;", "&")
}

/// `[0,63][1080,210]` → (0, 63, 1080, 210).
fn parse_bounds(text: &str) -> Option<(i32, i32, i32, i32)> {
    let numbers: Vec<i32> = text
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    match numbers[..] {
        [l, t, r, b] => Some((l, t, r, b)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node index="0" text="" resource-id="" class="android.widget.FrameLayout" content-desc="" clickable="false" bounds="[0,0][1080,2400]"><node index="0" text="Sign &amp; save" resource-id="com.app:id/save" class="android.widget.Button" content-desc="" clickable="true" bounds="[100,200][500,320]" /></node></hierarchy>"#;

    #[test]
    fn parses_nodes_and_finds_the_one_under_a_point() {
        let nodes = parse(XML);
        assert_eq!(nodes.len(), 2);
        let button = node_at(&nodes, 300, 250).unwrap();
        assert_eq!(button.text, "Sign & save");
        assert_eq!(button.center(), (300, 260));
        assert!(summary(&nodes).contains("Button \"Sign & save\" id=com.app:id/save clickable"));
        assert_eq!(
            node_at(&nodes, 900, 2000).unwrap().class,
            "android.widget.FrameLayout"
        );
    }

    #[test]
    fn a_recorded_tap_finds_its_element_on_another_screen() {
        let node = |id: &str, text: &str, bounds| UiNode {
            class: "android.widget.Button".into(),
            resource_id: id.into(),
            text: text.into(),
            description: String::new(),
            clickable: true,
            focused: false,
            password: false,
            bounds,
        };
        let nodes = vec![
            node("", "", (0, 0, 720, 1600)),
            node("com.app.debug:id/row", "Tea", (0, 100, 720, 200)),
            node("com.app.debug:id/row", "Coffee", (0, 200, 720, 300)),
            node("", "Coffee", (0, 900, 720, 1000)),
        ];
        assert_eq!(screen(&nodes), Some((720, 1600)));
        assert_eq!(texts(&nodes), "Tea\nCoffee\nCoffee");
        // By id, whichever build's; of rows sharing it, the one saying so.
        let row = Mark {
            id: "com.app:id/row".into(),
            text: "Coffee".into(),
            ..Mark::default()
        };
        assert_eq!(find(&nodes, &row, None).unwrap().bounds.1, 200);
        // Rows share the id and none says it: not another row, however near.
        let other = Mark {
            id: "com.app:id/row".into(),
            text: "Cocoa".into(),
            ..Mark::default()
        };
        assert!(find(&nodes, &other, Some((360, 150))).is_err());
        // An id of its own finds it when its words changed.
        let renamed = [node("com.app:id/save", "Save all", (0, 0, 100, 50))];
        let save = Mark {
            id: "com.app:id/save".into(),
            text: "Save".into(),
            ..Mark::default()
        };
        assert_eq!(find(&renamed, &save, None).unwrap().text, "Save all");
        // By words alone: the one nearest where it was tapped.
        let words = Mark {
            text: "Coffee".into(),
            ..Mark::default()
        };
        assert_eq!(
            find(&nodes, &words, Some((360, 950))).unwrap().bounds.1,
            900
        );
        let gone = Mark {
            text: "Cocoa".into(),
            ..Mark::default()
        };
        let err = find(&nodes, &gone, None).unwrap_err();
        assert!(err.contains("“Cocoa”") && err.contains("Tea"), "{err}");
        // A point recorded on a 1080×2400 screen, on this one.
        assert_eq!(scale((540, 1200), (1080, 2400), (720, 1600)), (360, 800));
        assert_eq!(scale((5, 5), (0, 0), (720, 1600)), (5, 5));
    }
}
