//! Captures the app's own window to a PNG. Reading back a window the process owns needs no Screen Recording grant.

use std::path::Path;

/// An RGBA image, row by row, at the display's scale.
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Image {
    /// A frame captured before the first paint comes back one flat color.
    pub fn is_uniform(&self) -> bool {
        self.rgba.chunks(4).all(|p| p == &self.rgba[..4])
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut png = png::Encoder::new(std::io::BufWriter::new(file), self.width as u32, self.height as u32);
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        let mut writer = png.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&self.rgba).map_err(|e| e.to_string())
    }
}

/// The visible window's content area, `content_height` points tall at `scale` pixels a point: the title bar above
/// it is cropped off, since it draws differently whenever the window is not key.
#[cfg(target_os = "macos")]
pub fn window(content_height: f32, scale: f32) -> Result<Image, String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGDataProvider, CGImage, CGWindowImageOption, CGWindowListOption};

    let mtm = MainThreadMarker::new().ok_or("capture runs on the main thread")?;
    let windows = NSApplication::sharedApplication(mtm).windows();
    let number = windows.iter().find(|w| w.isVisible()).ok_or("no visible window")?.windowNumber();
    // A null rect (an infinite origin) means the window's own bounds.
    let null = CGRect::new(CGPoint::new(f64::INFINITY, f64::INFINITY), CGSize::new(0.0, 0.0));
    #[allow(deprecated)]
    let image = objc2_core_graphics::CGWindowListCreateImage(
        null,
        CGWindowListOption::OptionIncludingWindow,
        number as u32,
        CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::BestResolution,
    )
    .ok_or("CGWindowListCreateImage returned nothing")?;
    let image = Some(&*image);
    let (width, height, row) = (CGImage::width(image), CGImage::height(image), CGImage::bytes_per_row(image));
    if CGImage::bits_per_pixel(image) != 32 {
        return Err(format!("unexpected {} bits per pixel", CGImage::bits_per_pixel(image)));
    }
    let data = CGDataProvider::data(CGImage::data_provider(image).as_deref()).ok_or("the image has no data")?;
    let bytes = data.to_vec();
    let top = height.saturating_sub((content_height * scale).round() as usize);
    let mut rgba = Vec::with_capacity(width * (height - top) * 4);
    for y in top..height {
        for p in bytes[y * row..y * row + width * 4].chunks(4) {
            rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    Ok(Image { width, height: height - top, rgba })
}

#[cfg(not(target_os = "macos"))]
pub fn window(_: f32, _: f32) -> Result<Image, String> {
    Err("--snapshot captures on macOS only".into())
}
