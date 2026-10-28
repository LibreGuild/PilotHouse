//! Font files: a specimen of the font at several sizes, its characters,
//! and what it says about itself.

use std::borrow::Cow;
use std::path::PathBuf;

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div, px,
};

use crate::byte_size;

const PANGRAM: &str = "The quick brown fox jumps over the lazy dog";
const SIZES: [f32; 6] = [12., 16., 20., 28., 40., 56.];
const CHARACTERS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ abcdefghijklmnopqrstuvwxyz 0123456789 !?&@#%*()[]{}<>/\\|+-=_~\"'.,:;";

#[derive(Debug, Clone, PartialEq)]
pub struct FontInfo {
    pub family: String,
    pub style: String,
    pub version: String,
    pub glyphs: u16,
    pub faces: u32,
    pub size: u64,
}

pub struct FontPreview {
    path: PathBuf,
    loaded: Option<Result<FontInfo, SharedString>>,
    _load: Option<Task<()>>,
}

impl FontPreview {
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            loaded: None,
            _load: None,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        let read = cx.background_spawn(async move {
            let bytes = std::fs::read(&path).map_err(|err| err.to_string())?;
            let info = read_info(&bytes)?;
            Ok::<_, String>((bytes, info))
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            _ = this.update(cx, |this, cx| {
                this.loaded = Some(match result {
                    Ok((bytes, info)) => {
                        // Registered with the app so the specimen draws in
                        // it; only a family the app does not know yet, so a
                        // file can never stand in for a font the interface
                        // uses (and reopening it adds nothing).
                        let known = cx
                            .text_system()
                            .all_font_names()
                            .iter()
                            .any(|name| name.eq_ignore_ascii_case(&info.family));
                        if known {
                            Ok(info)
                        } else {
                            match cx.text_system().add_fonts(vec![Cow::Owned(bytes)]) {
                                Ok(()) => Ok(info),
                                Err(err) => Err(format!("{err:#}").into()),
                            }
                        }
                    }
                    Err(err) => Err(err.into()),
                });
                cx.notify();
            });
        }));
    }
}

/// The first face's names and counts.
pub fn read_info(bytes: &[u8]) -> Result<FontInfo, String> {
    let faces = ttf_parser::fonts_in_collection(bytes).unwrap_or(1);
    let face = ttf_parser::Face::parse(bytes, 0).map_err(|err| format!("Not a font: {err}"))?;
    let name = |id: u16| {
        face.names()
            .into_iter()
            .filter(|name| name.name_id == id)
            .find_map(|name| name.to_string())
    };
    use ttf_parser::name_id;
    Ok(FontInfo {
        family: name(name_id::TYPOGRAPHIC_FAMILY)
            .or_else(|| name(name_id::FAMILY))
            .ok_or("The font has no family name.")?,
        style: name(name_id::TYPOGRAPHIC_SUBFAMILY)
            .or_else(|| name(name_id::SUBFAMILY))
            .unwrap_or_default(),
        version: name(name_id::VERSION).unwrap_or_default(),
        glyphs: face.number_of_glyphs(),
        faces,
        size: bytes.len() as u64,
    })
}

impl Render for FontPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let info = match &self.loaded {
            None => return crate::notice("Reading the font…", cx),
            Some(Err(err)) => return crate::notice(err.clone(), cx),
            Some(Ok(info)) => info.clone(),
        };
        let theme = cx.theme();
        let family: SharedString = info.family.clone().into();
        let mut details = vec![
            info.style.clone(),
            format!("{} glyphs", info.glyphs),
            byte_size(info.size),
        ];
        if info.faces > 1 {
            details.push(format!("{} faces (the first shown)", info.faces));
        }
        if !info.version.is_empty() {
            details.push(info.version.clone());
        }
        let sample = |size: f32, text: &str, cx: &App| {
            h_flex()
                .gap_4()
                .items_baseline()
                .child(
                    div()
                        .w(px(36.))
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{size:.0}")),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(family.clone())
                        .text_size(px(size))
                        .child(text.to_string()),
                )
        };
        div()
            .id("font-preview")
            .size_full()
            .overflow_y_scroll()
            .bg(theme.background)
            .child(
                v_flex()
                    .p_8()
                    .gap_6()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .font_family(family.clone())
                                    .text_size(px(64.))
                                    .child(format!("{} Aa", info.family)),
                            )
                            .child(
                                div().text_sm().text_color(theme.muted_foreground).child(
                                    details
                                        .into_iter()
                                        .filter(|d| !d.is_empty())
                                        .collect::<Vec<_>>()
                                        .join(" · "),
                                ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .children(SIZES.iter().map(|size| sample(*size, PANGRAM, cx))),
                    )
                    .child(
                        div()
                            .pt_4()
                            .border_t_1()
                            .border_color(theme.border)
                            .font_family(family.clone())
                            .text_size(px(28.))
                            .line_height(px(44.))
                            .child(CHARACTERS),
                    ),
            )
            .into_any_element()
    }
}
