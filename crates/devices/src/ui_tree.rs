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
}
