# UI style guide

PilotHouse is a tool people look at all day. Its own interface should recede and frame the work (chats, code, terminals, devices), and draw the eye only where something needs the user. Every UI change follows these rules; review screens against the checklist at the end.

Tokens live in `crates/theme/themes/pilothouse.json` (light and dark) and `crates/theme/src/preferences.rs` (the user's accent, fonts, sizes and corners on top). Never hard-code a colour in a view when a theme token covers the role.

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
| Accent | `ring`, `selection`, `caret`, `link`, `info` (the user's accent) | focus, text selection, links, "working" spinners, progress | backgrounds, icons for decoration, selected tabs |
| State | `success`, `warning`, `danger` | done / needs you / failed, git added / modified / deleted, errors | anything that is not state |

- A workspace's colour is a small mark beside its name, never a wash over chrome.
- Agents' brand marks identify the agent, at 14–18 px, and nowhere else.
- To tint, mix an existing token (`token.opacity(0.12)`), never a new hex.

## 2. Type

The interface size setting (`ui_font_size`, 15 by default, 18 on Windows) is the rem; every `text_*` step scales with it.

| Step | gpui | At 14 | Use it for |
|---|---|---|---|
| Reading | `text_base` | 14 | chat prose, the composer, dialog bodies |
| Rows | `text_sm` | 12.25 | list rows, tabs, tool rows in chat, buttons, form labels |
| Captions | `text_xs` | 10.5 | timestamps, counts, paths under a name, section labels |
| Code | `mono_font_family` + `text_xs`/`text_sm` | | code, diffs, commands, paths, branches, model ids |

- Section labels: `text_xs`, medium weight, muted. No other heading styles inside panels.
- Weight is for emphasis: `MEDIUM` for titles and the selected item, normal otherwise. No bold chrome.

## 3. Space and density

- 4 px rhythm (`gap_1` = 4 px at 14). Rows are 28 px (`h_7`), compact rows 24 px (`h_6`), sidebar cards two or three lines.
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
- Icons: 16 px (`small()`) in rows and toolbars, 14 px (`xsmall()`) beside text, inheriting text colour.
- Tooltips name icon-only controls. Never repeat a visible label; never put information only in a tooltip.

## 6. One way to each place

Each tool has one launcher on screen: the icon bar on the right. Panes show one tab strip; zoom, split and move live in the pane's ⋯ menu. Do not add a second switcher for the same thing.

## 7. Feedback

- Match feedback to duration: nothing under 100 ms; disabled under 1 s; a spinner from 1 to 3 s; a stage label ("Building… 12s") beyond.
- Copy never claims more than is known: "Installing…" while it runs, "Installed" only after.
- Empty states say what goes here and offer the action that fills them.
- Transient results are notifications; errors the user must read or act on stay inline.

## 8. Motion

Short and functional: expanding rows, a spinner, the working shimmer. Nothing decorative. Respect reduced motion.

## 9. Cross-platform

Every screen holds up on macOS, Windows and Linux, in light and dark, at interface sizes 12 to 24. Shortcut labels match the platform's real binding (`⌘` on macOS, `Ctrl+` elsewhere).

## Review checklist

1. Does colour appear anywhere it is not state, focus or selection?
2. Is there exactly one primary action, and is it obvious?
3. Are there cards inside cards, or two switchers for the same thing?
4. Do the text sizes come from the four steps?
5. Are secondary row actions on hover?
6. Can the common task be done in one or two actions, with Enter and Esc doing the expected thing?
7. Does it read in light mode and at interface size 18?
