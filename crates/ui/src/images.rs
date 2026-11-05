//! Letting go of pictures for good: their decoded pixels and the GPU
//! textures every window made of them.
//!
//! GPUI keeps both until told. Telling it from a view's release (a closed
//! tab) is not enough on its own: that runs while the window is busy, and
//! GPUI then skips that window, so its texture stayed. These wait until the
//! window is free.

use std::sync::Arc;

use std::rc::Rc;

use gpui_kit::{App, Global, Image, RenderImage};

/// How the app hands freed memory back to the system (set by the app at
/// start): called after pictures are let go, since the allocator keeps
/// what a large picture freed until asked.
#[derive(Clone)]
pub struct MemoryTrim(pub Rc<dyn Fn(&mut App)>);

impl Global for MemoryTrim {}

fn trim_soon(cx: &mut App) {
    if let Some(trim) = cx.try_global::<MemoryTrim>().cloned() {
        (trim.0)(cx);
    }
}

/// Drops `image`'s textures from every window, once they are free.
pub fn release_render_image(image: Arc<RenderImage>, cx: &mut App) {
    cx.defer(move |cx| {
        cx.drop_image(image, None);
        trim_soon(cx);
    });
}

/// Drops `images`' textures from every window, once they are free.
pub fn release_render_images(images: Vec<Arc<RenderImage>>, cx: &mut App) {
    if images.is_empty() {
        return;
    }
    cx.defer(move |cx| {
        for image in images {
            cx.drop_image(image, None);
        }
        trim_soon(cx);
    });
}

/// Lets go of an encoded picture shown with `img(image)`: its decoded
/// pixels (GPUI's asset cache) and its textures.
pub fn release_image(image: Arc<Image>, cx: &mut App) {
    cx.defer(move |cx| {
        // Its decoded pixels, as GPUI cached them (any window can ask).
        let render = cx.windows().into_iter().find_map(|window| {
            let image = image.clone();
            window
                .update(cx, |_, window, cx| image.get_render_image(window, cx))
                .ok()
                .flatten()
        });
        if let Some(render) = render {
            cx.drop_image(render, None);
        }
        image.remove_asset(cx);
        trim_soon(cx);
    });
}
