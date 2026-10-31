//! App icon sets: one picture made into every size Android, iOS, macOS and
//! websites ask for, in the folders and names their tools expect.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use image::{RgbaImage, imageops};

use crate::encode::{ico, square};

const ANDROID: [(&str, u32); 5] = [
    ("mdpi", 48),
    ("hdpi", 72),
    ("xhdpi", 96),
    ("xxhdpi", 144),
    ("xxxhdpi", 192),
];

/// macOS `.iconset` sizes: `iconutil -c icns AppIcon.iconset` makes the
/// `.icns`.
const MACOS: [u32; 5] = [16, 32, 128, 256, 512];

/// Writes the set into a new folder `AppIcons` (`AppIcons 2`… when taken)
/// in `parent`; returns it. A picture that is not square is centered on a
/// see-through square first.
pub fn write_icon_set(image: &RgbaImage, parent: &Path) -> Result<PathBuf> {
    let picture = square(image);
    let folder = free_name(parent, "AppIcons");
    let sized = |size: u32| imageops::resize(&picture, size, size, imageops::FilterType::Lanczos3);
    let save = |image: &RgbaImage, path: PathBuf| -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("could not make {}", dir.display()))?;
        }
        image
            .save_with_format(&path, image::ImageFormat::Png)
            .with_context(|| format!("could not write {}", path.display()))
    };

    for (density, size) in ANDROID {
        save(
            &sized(size),
            folder
                .join("android")
                .join(format!("mipmap-{density}"))
                .join("ic_launcher.png"),
        )?;
    }
    save(
        &sized(512),
        folder.join("android").join("play-store-512.png"),
    )?;

    // The App Store refuses icons with transparency.
    let ios = folder.join("ios").join("AppIcon.appiconset");
    save(&on_white(&sized(1024)), ios.join("AppIcon-1024.png"))?;
    std::fs::write(
        ios.join("Contents.json"),
        r#"{
  "images" : [
    {
      "filename" : "AppIcon-1024.png",
      "idiom" : "universal",
      "platform" : "ios",
      "size" : "1024x1024"
    }
  ],
  "info" : {
    "author" : "xcode",
    "version" : 1
  }
}
"#,
    )
    .context("could not write Contents.json")?;

    let iconset = folder.join("macos").join("AppIcon.iconset");
    for size in MACOS {
        save(
            &sized(size),
            iconset.join(format!("icon_{size}x{size}.png")),
        )?;
        save(
            &sized(size * 2),
            iconset.join(format!("icon_{size}x{size}@2x.png")),
        )?;
    }

    let web = folder.join("web");
    std::fs::create_dir_all(&web).context("could not make the web folder")?;
    std::fs::write(web.join("favicon.ico"), ico(&picture, &[16, 32, 48])?)
        .context("could not write favicon.ico")?;
    save(&sized(180), web.join("apple-touch-icon.png"))?;
    save(&sized(192), web.join("icon-192.png"))?;
    save(&sized(512), web.join("icon-512.png"))?;
    Ok(folder)
}

/// `name` in `parent`, numbered when taken.
fn free_name(parent: &Path, name: &str) -> PathBuf {
    let mut path = parent.join(name);
    let mut number = 2;
    while path.exists() {
        path = parent.join(format!("{name} {number}"));
        number += 1;
    }
    path
}

fn on_white(image: &RgbaImage) -> RgbaImage {
    let mut out = RgbaImage::from_pixel(image.width(), image.height(), image::Rgba([255; 4]));
    imageops::overlay(&mut out, image, 0, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_every_platform_and_never_over_a_folder() {
        let dir = std::env::temp_dir().join(format!("ph-studio-icons-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let picture = RgbaImage::from_pixel(300, 200, image::Rgba([30, 60, 200, 255]));
        let first = write_icon_set(&picture, &dir).unwrap();
        let second = write_icon_set(&picture, &dir).unwrap();
        assert_ne!(first, second);
        for file in [
            "android/mipmap-xxxhdpi/ic_launcher.png",
            "ios/AppIcon.appiconset/Contents.json",
            "macos/AppIcon.iconset/icon_512x512@2x.png",
            "web/favicon.ico",
        ] {
            assert!(first.join(file).exists(), "{file}");
        }
        let ios = image::open(first.join("ios/AppIcon.appiconset/AppIcon-1024.png")).unwrap();
        assert_eq!((ios.width(), ios.height()), (1024, 1024));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
