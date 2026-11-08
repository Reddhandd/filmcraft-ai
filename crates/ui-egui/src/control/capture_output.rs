//! Bounded pixel extraction and disk-free PNG delivery for model inspection.
use egui::ColorImage;
use image::ImageEncoder;
use serde_json::{Value, json};

const MAX_RAW: usize = 128 * 1024 * 1024;
// Base64 plus metadata stays below the Console's 384 KiB image-tool budget.
const MAX_PNG: usize = 256 * 1024;

pub(super) fn crop_rgba(image: &ColorImage, (x, y, width, height): (usize, usize, usize, usize)) -> Result<Vec<u8>, String> {
    let [iw, ih] = image.size;
    let pixels = iw.checked_mul(ih).ok_or("Screenshot dimensions overflow")?;
    if pixels != image.pixels.len() || pixels > MAX_RAW / 4 {
        return Err("Screenshot pixels do not match its dimensions or exceed the image budget".into());
    }
    let right = x.checked_add(width).filter(|v| *v <= iw).ok_or("Screenshot crop exceeds the image width")?;
    let bottom = y.checked_add(height).filter(|v| *v <= ih).ok_or("Screenshot crop exceeds the image height")?;
    let capacity = width.checked_mul(height).and_then(|n| n.checked_mul(4)).filter(|n| *n <= MAX_RAW).ok_or("Screenshot crop exceeds the image budget")?;
    if capacity == 0 {
        return Err("Screenshot crop is empty".into());
    }
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(capacity).map_err(|_| "Not enough memory for screenshot pixels")?;
    // Dimensions and the complete backing buffer were validated above.
    for row in y..bottom {
        for col in x..right {
            let color = image.pixels[row * iw + col];
            rgba.extend_from_slice(&[color.r(), color.g(), color.b(), 255]);
        }
    }
    Ok(rgba)
}

fn encode(image: &image::RgbaImage) -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(&mut png, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Adaptive)
        .write_image(image.as_raw(), image.width(), image.height(), image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("Cannot encode screenshot: {e}"))?;
    Ok(png)
}

pub(super) fn inline(image: &ColorImage, crop: Option<[f32; 4]>, ppp: f32, max_side: u32) -> Result<Value, String> {
    if !(64..=4096).contains(&max_side) {
        return Err("maxSide must be an integer from 64 to 4096".into());
    }
    let ppp = if ppp.is_finite() && ppp > 0.0 { ppp } else { 1.0 };
    let rect = super::crop_pixels(image.size, crop, ppp)?;
    let width = u32::try_from(rect.2).map_err(|_| "Screenshot width exceeds its budget")?;
    let height = u32::try_from(rect.3).map_err(|_| "Screenshot height exceeds its budget")?;
    let original = image::RgbaImage::from_raw(width, height, crop_rgba(image, rect)?).ok_or("Screenshot has invalid pixels")?;
    let mut side = max_side.min(width.max(height));
    // Each step shrinks by 25%; 16 steps suffice for the accepted 4096-side limit.
    for _ in 0..16 {
        let scale = f64::from(side) / f64::from(width.max(height));
        let w = (f64::from(width) * scale).round().max(1.0) as u32;
        let h = (f64::from(height) * scale).round().max(1.0) as u32;
        let resized;
        let pixels = if (w, h) == (width, height) {
            &original
        } else {
            resized = image::imageops::resize(&original, w, h, image::imageops::FilterType::Triangle);
            &resized
        };
        let png = encode(pixels)?;
        if png.len() <= MAX_PNG {
            return Ok(
                json!({"pngBase64":super::base64(&png),"width":w,"height":h,"originalWidth":width,"originalHeight":height,"retainedFiles":0,"delivery":"inline"}),
            );
        }
        if side <= 64 {
            break;
        }
        side = (side.saturating_mul(3) / 4).max(64);
    }
    Err("Screenshot could not fit its delivery budget; capture a smaller panel".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_backing_pixels_and_hostile_crops_fail_without_panicking() {
        let mut image = ColorImage::filled([2, 2], egui::Color32::RED);
        image.pixels.pop();
        assert!(std::panic::catch_unwind(|| inline(&image, None, 1.0, 960)).unwrap().is_err());
        let legacy = std::panic::catch_unwind(|| super::super::save_screenshot_with_ppp(&image, None, None, 1.0)).unwrap();
        assert_eq!(legacy["ok"], false);
        let image = ColorImage::filled([2, 2], egui::Color32::RED);
        for crop in [Some([f32::NAN, 0.0, 1.0, 1.0]), Some([100.0, 100.0, 1.0, 1.0])] {
            assert!(inline(&image, crop, 1.0, 960).is_err());
        }
        assert!(inline(&image, None, 1.0, u32::MAX).is_err());
    }
    #[test]
    fn inline_png_is_bounded_and_needs_no_filesystem() {
        let mut image = ColorImage::filled([800, 400], egui::Color32::BLACK);
        for (i, color) in image.pixels.iter_mut().enumerate() {
            *color = egui::Color32::from_rgb((i % 251) as u8, ((i / 7) % 253) as u8, ((i / 13) % 255) as u8);
        }
        let result = inline(&image, None, 2.0, 400).unwrap();
        assert!(result["width"].as_u64().unwrap() <= 400);
        assert_eq!(result["retainedFiles"], 0);
        assert!(result.get("path").is_none());
        assert!(serde_json::to_vec(&result).unwrap().len() < 384 * 1024);
        assert!(result["pngBase64"].as_str().unwrap().starts_with("iVBORw0KGgo"));
    }
}
