//! What an agent is told about boards: read once (`board_guide`) before
//! its first change to one.

/// The guide.
pub fn text() -> String {
    format!(
        r#"# A {app} whiteboard

A board is one file, `site/board.json`, in the board's folder. The file is the truth: the user sees every change you write at once, and what they change by hand is written to the same file. Read it before you change it (they may have moved things), write it whole and valid, and keep the ids of things that stay: arrows and the user's selection hold on to them.

```json
{{ "version": 1, "title": "Launch plan", "elements": [ … ] }}
```

`elements` are drawn in order, the last on top. Places are in board points: `x`, `y` is an element's top left corner, `w`, `h` its size; x grows to the right, y downwards. The board has no edge. Every element has an `id` (short, unique, yours to choose: `n1`, `step-build`) and a `type`:

| type | fields | what |
|---|---|---|
| `sticky` | x, y, w, h, text, color | A sticky note (200 × 200 is usual). One idea a note, a few words. |
| `shape` | x, y, w, h, shape, text, fill, stroke | `shape`: rect, rounded, ellipse, pill, diamond, triangle, hexagon, slant, cylinder, document, callout, cloud, person, star. `fill` and `stroke` are colour names, or `none`. |
| `text` | x, y, w, text, size, color | Words alone. `w` is the width they wrap at. |
| `frame` | x, y, w, h, title, color | A section: a titled area that holds what lies inside it. Put it before its contents in the list. |
| `line` | from, to, route, start, end, dash, color, text | An arrow or a line. `from` and `to` are {{ "id": "n1" }} (it then follows that element; add "side": top, right, bottom or left to choose where it meets it) or {{ "x": 0, "y": 0 }}. `route`: straight, elbow or curve. `start`, `end`: none, arrow or dot (default: end is arrow). `text` labels it. |
| `table` | x, y, w, h, rows | `rows` is a list of rows, each a list of cell texts; the first row is the heading. |
| `sticker` | x, y, w, h, emoji | One emoji, large: a stamp of approval, a warning. |
| `image` | x, y, w, h, src | A picture in `media/` (the user adds those). |
| `draw` | points, color, width | A freehand line; `points` are [x, y] pairs. Rarely yours to make. |
| `comment` | x, y, text, by, replies, resolved | A note pinned to the board. To answer one, add {{ "by": "Agent", "text": "…" }} to its `replies`, and set `resolved` when it is done. |

Text fields common to sticky, shape and text: `size` (s, m, l, xl, xxl; default m), `bold`, `italic`, `align` (left, center, right), `list` (true: each line a bullet). A shape's outline and a line may have `dash` (true). A sticky may have `votes` (a number: the dots people gave it; theirs, leave it) and `by` (who wrote it). Any element may have `locked` (true: the user fixed it in place; leave it where it is) and `group` (elements with the same value move together).

Colours are names, the same in light and dark: yellow, orange, red, pink, purple, blue, teal, green, gray, ink (the text colour), and for a shape's fill also `none`. Use colour to mean something (one colour a theme, a team, a state) and say what in a legend when it is not obvious.

## Laying things out

- Leave room: 40 points between neighbours, 80 between groups. Align things on a grid of 20; rows and columns that line up read at a glance.
- A diagram reads left to right or top to bottom. Give every step a shape and every relation a `line` with ids at both ends, so it stays attached when the user moves things.
- Shapes say what a thing is: `rounded` a step, `diamond` a decision, `cylinder` a store of data, `document` a file, `person` someone, `cloud` something outside, `pill` a start or an end.
- Size a shape to its words: about 9 points a letter at size m, 24 points a line, with 16 points around. A sticky holds about 12 words.
- Group related notes in a `frame` with a title; a board of fifty loose notes is not read.
- Do not delete what the user made unless they ask. Add beside it, and say where.

After you change the board, call `board_look`: it says what is on the board as the file reads, what the user has selected ("this" in their message means that), and whether they changed it by hand since."#,
        app = ph_brand::NAME
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_guide_names_the_file_and_every_kind_of_thing() {
        let text = super::text();
        for word in [
            "board.json",
            "`sticky`",
            "`shape`",
            "`frame`",
            "`line`",
            "`table`",
            "`comment`",
            "board_look",
        ] {
            assert!(text.contains(word), "{word}");
        }
    }
}
