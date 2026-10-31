# Themes

PilotHouse comes with eight themes, and you can design your own by describing it to an agent: the app changes as you talk, and you keep the result as a theme of your own.

## Choosing one

**Settings → Appearance → Theme** lists them:

| Theme | Mode | After |
|---|---|---|
| Match system | follows the system | Modern Light or Modern Dark, as your computer is (the default) |
| Modern Dark | dark | VS Code's Dark Modern greys |
| Modern Light | light | a clean white with a blue accent |
| Warm Dark | dark | a soft dark brown, easier on the eyes |
| Warm Light | light | warm paper and ink |
| Dracula | dark | the Dracula theme (draculatheme.com) |
| One Dark | dark | Atom's One Dark |
| One Light | light | Atom's One Light |
| Studio Dark | dark | Android Studio's and IntelliJ's dark look |

Themes you made are listed after them, marked *(yours)*. *Use light appearance* and *Use dark appearance* in the command palette switch to the theme's light or dark version (Modern, Warm and One come in both), else to Modern Light or Modern Dark. Fonts, sizes, corners and a translucent window are settings of their own, kept whichever theme you choose.

A choice saved by an earlier version (Light, or Dark with Modern or Warm) becomes the theme it showed, once.

## Making it yours

All in **Settings → Appearance**; the defaults are the look PilotHouse has always had.

- **Accent**: one color for focus outlines, links, selected text, highlights and progress, laid over whichever theme is chosen (the first, empty swatch is the theme's own). The main buttons take it too when the theme draws them in its accent (Modern Dark's Send), not when they are its own color (Modern Light's black). Each color comes in a shade that reads on a dark theme and one for a light theme.
- **Density** (*Layout*): Compact, Comfortable or Spacious: the height of rows and the spacing in the sidebar, the file tree, the Git lists and the chat. Text keeps its size (that is *Interface size*).
- **Terminal colors** follow the theme: Dracula's terminal looks like Dracula. The themes from earlier versions keep the terminal as it was; a theme file sets its own with `terminal`.
- **Background** (*Window*): Solid, Translucent (the desktop, blurred), Mica on Windows, a **Gradient** (Dusk, Ocean, Aurora, Ember, Graphite; deep on a dark theme, soft on a light one) or an **Image** of yours. A gradient or image shows behind the frame (sidebar, title bar, the strips around the panels) under a veil of the frame's color, so its text stays readable; the panels keep *Panel opacity*. PilotHouse keeps its own copy of the image, scaled to a screen's size off the UI thread, so a large photo costs a screen's worth of memory.

## Designing your own

**Design with an agent** (under the theme, or *Design a theme with an agent* in the command palette) opens a new chat with the message begun: finish it with the look you want ("a deep navy frame with amber buttons, softer text, a green-on-black editor"), in as many messages as you like. The agent shows each change at once, in every window, and saves it under a name when you say so. Until then nothing is kept: closing PilotHouse, or picking a theme in Settings, drops the preview.

The agent works with the `theme_*` tools of PilotHouse's MCP server:

| Tool | Does |
|---|---|
| `theme_get` | The theme shown (every color token, the editor's colors) and a guide to where each part of PilotHouse takes its colors from |
| `theme_preview` | Shows changes at once: part of a theme, laid over the one shown (or over another with `from`). Misspelled tokens and colors that cannot be read are refused, with what was wrong |
| `theme_save` | Saves the preview under a name and uses it; saving again under the same name updates it |
| `theme_discard` | Drops the preview |
| `theme_list`, `theme_use` | The themes, and which is in use; use one (or `system`) |
| `theme_import` | Imports a VS Code color theme from its JSON (the agent can fetch it from a link first) |
| `theme_delete` | Deletes one of yours |

## Importing a VS Code theme

**Import…** (under the theme) takes a VS Code color theme file (`.json`, comments and trailing commas allowed): its workbench colors (editor, side bar, title and status bars, lists, buttons, inputs, menus), its code colors (matched by TextMate scope: an exact scope first, then a narrower one) and its terminal colors become one of your themes, saved and in use. What the file leaves out comes from Modern Dark or Modern Light, by its type. An agent can import from a link too (`theme_import`).

## Theme files

Your themes are JSON files in `<data dir>/themes` (**Open folder** in Settings shows it), in gpui-component's theme format: a name and a list of themes, each with a `mode`, its `colors` (`background`, `sidebar.background`, `primary.background`, `muted.foreground`…) and its editor colors under `highlight` (`editor.background`, `syntax.keyword.color`…). A theme can also set the terminal's 16 colors: `"terminal": {"black": "#…", "red": "#…", …, "bright_white": "#…"}`. You can edit them by hand or share them; Settings reads them again each time it opens. A theme cannot take the name of one of PilotHouse's own.

## Code map

| File | What |
|---|---|
| `crates/theme/themes/pilothouse.json` | PilotHouse's eight themes |
| `crates/theme/src/themes.rs` | The themes to choose from (PilotHouse's and yours), the selected one, the preview, saving and deleting |
| `crates/theme/src/appearance.rs` | The choice (`Appearance`: the system, or a theme by name) and showing it |
| `crates/theme/src/preferences.rs` | Fonts, sizes, corners, accent, density and the window's background, laid over the theme |
| `crates/theme/src/vscode.rs` | Importing VS Code color themes |
| `crates/terminal/src/palette.rs` | The terminal's colors: the theme's, else its palette for the mode |
| `crates/workspace/src/theme_mods.rs` | The `theme.*` control methods behind the tools |
| `crates/mcp/src/tools/theme.rs` | The `theme_*` MCP tools |
