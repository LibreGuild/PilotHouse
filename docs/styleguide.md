# UI style guide

PilotHouse is a tool people look at all day. Its own interface should recede and frame the work (chats, code, terminals, devices), and draw the eye only where something needs the user. Every UI change follows these rules; review screens against the checklist at the end.

Tokens live in `crates/theme/themes/pilothouse.json` (light and dark) and `crates/theme/src/preferences.rs` (the brand accent, and the user's fonts, sizes and corners on top). Never hard-code a colour in a view when a theme token covers the role.

## 1. Colour: quiet chrome, loud state

The look is a calm graphite, cool rather than warm: near-black with a faint blue cast in dark mode, white with cool greys in light mode. Colour is a signal, so it is rare.

| Role | Token | Use it for | Never for |
|---|---|---|---|
| Canvas | `background` | chat, editor, terminal, tool panels: the content | |
| Chrome | `sidebar`, `title_bar`, `status_bar`, `tab_bar` | the frame around the content, a step darker (dark) or greyer (light) than the canvas | content |
| Floating | `popover` | menus, popovers, dialogs | inline UI |
| Fills | `muted` | inputs, chips, code spans, quiet blocks | body text |
| Hover | `list_hover`, `secondary_hover` | rows and ghost buttons under the pointer | persistent state |
| Selected row | `list_active` | the current row: a neutral lift, no colour | |
| Hairlines | `border` | every divider and outline | emphasis |
| Text | `foreground`, `muted_foreground` | text; secondary text, captions, paths | |
| The one action | `primary` | the single affirmative button of a surface (Send, Commit, Save): near-white on dark, near-black on light | decoration, secondary actions |
| Accent | `ring` (focus, a border that marks), `link` (links, and an icon or a word tinted to draw the eye), `selection`, `progress_bar`, and `primary` on what shows a choice (a switch, a checked box, a tab's line): the theme's own, a plain blue as in VS Code (`#0078d4` dark, `#005fb8` light), or the accent chosen in Settings → Appearance, which is laid over all of these. The logo's violet is the logo's only, never the UI's | focus, text selection, links, progress, the thing to click in a drawing | backgrounds, selected tabs, a state |
| State | `success`, `warning`, `danger`, `info` | done / needs you / failed / working, git added / modified / deleted / renamed, errors and notes. They are the theme's and never follow the accent: a green accent must not make *working* look like *done* | anything that is not state, a tint that should follow the accent (that is `link`) |

- A workspace's colour is a small mark beside its name, never a wash over chrome.
- Agents' brand marks identify the agent, at 14–18 px, and nowhere else.
- **The mark**: a white mast and pennant, the pennant the P's bowl (a signal flag: someone is steering), on a violet glass tile (`#A78BFA` → `#6D28D9` → `#3B0FA8`, a soft pink glow at the top right). Sources in `brand/icons/` (`app.svg` on Apple's icon grid, `app-tile.svg` edge to edge). The title bar shows the tile at 20 px.
- To tint, mix an existing token (`token.opacity(0.12)`), never a new hex.
- **Contrast** (WCAG 2.1 AA): text is at least 4.5:1 on every surface it sits on (`foreground`, `muted_foreground` on canvas, chrome, fills and popovers; a filled button's label on its fill; links), state colours and the focus ring at least 3:1. Every bundled theme holds this. Muted text is the quietest text there is: do not dim it further (`muted_foreground.opacity(…)`) for anything that is read. Input borders are hairlines, quieter than 3:1; the focus ring is what marks the field in use.

## 2. Type

The interface size setting (`ui_font_size`, 16 by default on every system) is the rem; every `text_*` step scales with it. At 16 each step is a whole pixel, also on a screen at 150 % or 200 %; the numbers in this guide are for that size.

| Step | gpui | At 16 | Use it for |
|---|---|---|---|
| Reading | `text_base` | 16 | chat prose, the composer, dialog bodies |
| Rows | `text_sm` | 14 | list rows, tabs, tool rows in chat, buttons, form labels |
| Captions | `text_xs` | 12 | timestamps, counts, paths under a name, section labels |
| Code | `mono_font_family` + `text_xs`/`text_sm` | | code, diffs, commands, paths, branches, model ids |

- The fonts are PilotHouse's own on every system: Inter for the interface, JetBrains Mono for code, in its cut without ligatures (`crates/theme/fonts`, loaded by `ph_theme::fonts`). The same text takes the same room on macOS, Windows and Linux; a user can still pick another family, or the system's.

- Section labels: `text_xs`, medium weight, muted. No other heading styles inside panels.
- Weight is for emphasis: `MEDIUM` for titles and the selected item, normal otherwise. No bold chrome.

## 3. Space and density

- 4 px rhythm (`gap_1` = 4 px). Rows are 28 px (`h_7`), compact rows 24 px (`h_6`), Bridge cards two to four lines.
- **Everything follows the interface size**, as a web page does when it is zoomed: text, rows, icons, marks, paddings, dialogs. Use the rem helpers (`h_7()`, `gap_2()`, `size_4()`), and for a size off that scale `ph_ui::rems_from_px(14.)` (or `ph_ui::scaled_px(560., window)` where only pixels are taken, such as a dialog's width). A length in `px(…)` stays as it is at every size, so it is only for what must not scale: hairlines and strokes, corner radii, a window's size and the limits of its panes (`crates/theme/src/layout.rs`), and marks drawn on a picture.
- Panels run edge to edge; padding belongs inside lists (`px_2`), not around panels.
- Line up text to a visible left edge; right-align numbers, counts, shortcuts and ages.

## 4. Elevation and containers

Three levels, no fourth:

1. **Hairline** (`border_1` + `border`): the default for everything that needs an edge.
2. **Hairline + lift**: embedded blocks such as a diff under a file row, a question card in chat.
3. **Floating** (`shadow_lg`, `popover`): menus, popovers, dialogs, the composer.

**No cards inside cards.** A list of files is rows, not boxes; one bordered block under a row is the most nesting allowed.

## 5. Controls

- **One primary button per surface.** Everything else is `ghost` (icon buttons, row actions) or `outline` (a secondary action beside the primary).
- **Cancel, Close, Dismiss are quiet**: ghost, no colour, no shortcut chip. Esc always backs out.
- **Row actions appear on hover** (`invisible()` + `group_hover`), except the one action a row exists for.
- **Destructive** (`danger`) only for irreversible actions, and they ask first.
- Icons are at least as large as the text beside them: 14 to 16 px (`small()`, or `size_4()`) with row text, 12 to 14 px (`xsmall()`, `small()`) with captions, 18 px in the columns of icons; they inherit the text's colour.
- Tooltips name icon-only controls. Never repeat a visible label; never put information only in a tooltip.
- **A tooltip grows downwards, not sideways.** Its text is at most 52 characters a line (`ph_ui::tip`): broken at spaces, and inside a word longer than a line (a path, a URL) after a separator. Pass every tooltip whose text is made up as the app runs (a title, a path, a command, a state) through `ph_ui::tip(…)`, and any fixed one longer than a line; a short fixed label needs nothing.
- **A row of Settings** is a name, one sentence under it, and its control at the right. A control wider than a dropdown (several buttons, a sentence) goes under the text (`.layout(Axis::Vertical)`), or it runs off the page. A group's description shows only under a title. A page that is one view (`view_page` in `settings_dialog.rs`) lists the words Search finds it by. A form that fills a page keeps its way back and its Save over its fields, not under them.

## 6. One way to each place

Each place has one way in on screen: the column of icons for what the window shows (the Bridge, Reviews, a workspace), the row of names over the result pane for a workspace's tools. Do not add a second switcher for the same thing.

## 7. Feedback

- Match feedback to duration: nothing under 100 ms; disabled under 1 s; the working mark from 1 to 3 s; a stage label ("Building… 12s") beyond.
- **The working mark** is `ph_ui::Loader`: a pennant of four bars that flutters, after the app's icon, in the icon's own colours: violet at the mast running through purple and fuchsia to a hot pink at the fly (`loader.rs`, `HUES`). One family of hue, so it reads as one mark and as this app's; four different hues side by side read as other products' marks. The colours keep clear of red, amber, green and blue, which mean something here. A colour passed in (`.color(…)`, on a filled button) is every bar's. Use it wherever something is under way (`xsmall()` beside captions, `small()` beside row text, plain over a label, `large()` alone in a pane); a bare 12 px spinner went unnoticed. It stands still when the system asks for less motion.
- Copy never claims more than is known: "Installing…" while it runs, "Installed" only after.
- Empty states say what goes here and offer the action that fills them.
- Transient results are notifications; errors the user must read or act on stay inline.

## 8. Motion

Short and functional: expanding rows, a spinner, the working shimmer. Nothing decorative. Respect reduced motion.

## 9. The frame

The window is a frame with islands in it (`crates/workspace/src/frame.rs`; the [workspace page](features/workspace.md) says what each is):

- **The frame** is the title bar, the column of icons and the status bar. It is painted once, behind them all; none of them has a fill or a border of its own, and nothing draws a line across it. Its paint is the theme's `sidebar` colour, a little lighter at the top than at the bottom, and over it the colour of the workspace in front, faint, fading downwards.
- **An island** is `frame::island(window, cx)`: a `ph_ui::Surface` (rounded by `radius_lg`, hairline border, the theme's `background`) that knows what is behind its corners. Whatever a view shows between the frame's parts goes in one; islands are `frame::gap()` apart and end the same distance over the status bar. Do not round or fill the corners of what is inside: the island does it.
- A workspace's colour belongs to the frame, its badge (beside its name in the title bar, and in the list of workspaces) and its dot. Do not use it for fills, buttons or text inside an island.

## 10. Room

A pane is as narrow as 320 px (`CONTEXT_PANEL_MIN`; less in a small window) and as wide as the window, and every view holds up across that:

- Text in a row gives way before it is cut: `div().flex_1().min_w_0().truncate()` for a name, `min_w_0()` without `truncate()` for a sentence that should wrap. A bare string in an `h_flex` does neither.
- A row of named buttons folds to icons when the pane is narrow (`ph_ui::measure::narrower_than`, the breakpoint in pixels at the default interface size); each keeps its name in its tooltip and `accessibility_label`.
- A row of tabs is `ph_ui::PillTabs`: the tabs not in front fold to icons.
- **What scrolls shows a scrollbar.** `overflow_y_scrollbar()` (or `overflow_x_scrollbar()`, `overflow_scrollbar()`) in place of the plain `overflow_*_scroll()`; for an area that keeps its own scroll handle, the area in a `relative()` wrapper with `.vertical_scrollbar(&handle)` on the wrapper. Scrollbars stay in sight on Windows and Linux, and on a Mac set to always show them (`ph_theme::preferences::apply`): a mouse without a sideways wheel has no other way across a wide table. (An element that calls `.test_support()` after its scroll cannot take the first form.)
- A row that still cannot fit wraps (`flex_wrap`) with its main action at the right, or scrolls sideways (code, diffs: `ph_preview::diff::scroller`). Nothing is pushed out of the pane.
- A size that is known to the layout as a fixed width counts as the least its row can be: give what should shrink a `flex_basis` with `min_w_0()`, not a width.

## 11. Cross-platform

Every screen holds up on macOS, Windows and Linux, in light and dark, at interface sizes 11 to 24. The fonts, the sizes and the spacing are the same on all three: nothing in a view depends on the system (no `cfg!(windows)` for a size or a padding). Shortcut labels match the platform's real binding (`⌘` on macOS, `Ctrl+` elsewhere).

## Review checklist

1. Does colour appear anywhere it is not state, focus or selection?
2. Is there exactly one primary action, and is it obvious?
3. Are there cards inside cards, or two switchers for the same thing?
4. Do the text sizes come from the four steps?
5. Are secondary row actions on hover?
6. Can the common task be done in one or two actions, with Enter and Esc doing the expected thing?
7. Does it read in light mode, and at interface sizes 12 and 24 (do the icons, paddings and dialogs grow with the text)?
