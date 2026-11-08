//! Bounded font-picker masks with temporary font bytes and shaping data.
//! This path never registers faces or fills the permanent font/layout/glyph caches.
#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]
use crate::{Mask, Path};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
mod face;

fn preview(reader: &mut (impl std::io::Read + std::io::Seek), index: u32, label: &str, px: f32, width: usize, height: usize) -> Result<Mask, &'static str> {
    let bytes = face::extract(reader, index)?;
    raster(&bytes, label, px, width, height)
}

/// A failed preview leaves the picker usable; no fallback is claimed to be the requested face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewError {
    InvalidInput,
    MissingFamily,
    UnreadableFont,
    InvalidFont(&'static str),
}
impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidInput => f.write_str("Font preview exceeds its input or image budget"),
            Self::MissingFamily => f.write_str("Font family is unavailable"),
            Self::UnreadableFont => f.write_str("Font file is unavailable or is not a regular file"),
            Self::InvalidFont(reason) => write!(f, "Font preview unavailable: {reason}"),
        }
    }
}
impl std::error::Error for PreviewError {}

/// Render the actual family name. Call on a bounded worker for file-backed system fonts.
/// The catalog must already be populated; this function never triggers a system scan.
/// Temporary per-face bytes are capped at 64 MiB, and the returned mask at 960 by 96 pixels.
pub fn family(family: &str, px: f32, width: usize, height: usize) -> Result<Mask, PreviewError> {
    if family.is_empty()
        || family.len() > 512
        || family.chars().any(char::is_control)
        || !px.is_finite()
        || !(4.0..=64.0).contains(&px)
        || width == 0
        || width > 960
        || height == 0
        || height > 96
    {
        return Err(PreviewError::InvalidInput);
    }
    let info = crate::fonts::preview_info(family).ok_or(PreviewError::MissingFamily)?;
    let label: String = family.chars().take(80).collect();
    let result = match info.data {
        crate::fonts::FaceData::Static(bytes) => preview(&mut std::io::Cursor::new(bytes), info.index, &label, px, width, height),
        crate::fonts::FaceData::Shared(bytes) => preview(&mut std::io::Cursor::new(bytes.as_slice()), info.index, &label, px, width, height),
        crate::fonts::FaceData::File(path) => {
            if !std::fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                return Err(PreviewError::UnreadableFont);
            }
            let mut file = std::fs::File::open(path).map_err(|_| PreviewError::UnreadableFont)?;
            if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
                return Err(PreviewError::UnreadableFont);
            }
            preview(&mut file, info.index, &label, px, width, height)
        }
    };
    result.map_err(PreviewError::InvalidFont)
}

struct Pen {
    path: Path,
    x: f32,
    y: f32,
    commands: usize,
    failed: bool,
}
impl Pen {
    fn point(&mut self, x: f32, y: f32) -> Option<(f32, f32)> {
        let x = self.x + x;
        let y = self.y - y;
        if !x.is_finite() || !y.is_finite() || x.abs() > 100_000.0 || y.abs() > 100_000.0 {
            self.failed = true;
            None
        } else {
            Some((x, y))
        }
    }
    fn permit(&mut self) -> bool {
        self.commands = self.commands.saturating_add(1);
        if self.commands > 8192 || self.path.contours.iter().map(Vec::len).sum::<usize>() > 65536 {
            self.failed = true;
        }
        !self.failed
    }
}
impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.permit()
            && let Some((x, y)) = self.point(x, y)
        {
            self.path.move_to(x, y);
        }
    }
    fn line_to(&mut self, x: f32, y: f32) {
        if self.permit()
            && let Some((x, y)) = self.point(x, y)
        {
            self.path.line_to(x, y);
        }
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        if self.permit() {
            let a = self.point(cx, cy);
            let b = self.point(x, y);
            if let (Some(a), Some(b)) = (a, b) {
                self.path.quad_to(a.0, a.1, b.0, b.1);
            }
        }
    }
    fn curve_to(&mut self, ax: f32, ay: f32, bx: f32, by: f32, x: f32, y: f32) {
        if self.permit() {
            let a = self.point(ax, ay);
            let b = self.point(bx, by);
            let c = self.point(x, y);
            if let (Some(a), Some(b), Some(c)) = (a, b, c) {
                self.path.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
            }
        }
    }
    fn close(&mut self) {
        if self.permit() {
            self.path.close();
        }
    }
}

fn raster(bytes: &[u8], label: &str, px: f32, width: usize, height: usize) -> Result<Mask, &'static str> {
    if bytes.len() > 64 * 1024 * 1024
        || label.is_empty()
        || label.len() > 512
        || label.chars().count() > 80
        || label.chars().any(char::is_control)
        || !px.is_finite()
        || !(4.0..=64.0).contains(&px)
        || width == 0
        || width > 960
        || height == 0
        || height > 96
    {
        return Err("preview input exceeds budget");
    }
    let font = FontRef::from_index(bytes, 0).map_err(|_| "invalid font face")?;
    let metrics = font.metrics(Size::new(px), LocationRef::default());
    let em = font.metrics(Size::unscaled(), LocationRef::default()).units_per_em;
    if em == 0 || !metrics.ascent.is_finite() || !metrics.descent.is_finite() {
        return Err("invalid font metrics");
    }
    let k = px / f32::from(em);
    let shaper_data = harfrust::ShaperData::new(&font);
    let shaper = shaper_data.shaper(&font).build();
    let bidi = unicode_bidi::BidiInfo::new(label, None);
    let paragraph = bidi.paragraphs.first().ok_or("empty label")?;
    let (levels, runs) = bidi.visual_runs(paragraph, 0..label.len());
    let baseline = (height as f32 - (metrics.ascent - metrics.descent)) * 0.5 + metrics.ascent;
    if !baseline.is_finite() {
        return Err("invalid preview baseline");
    }
    let mut pen = Pen { path: Path::new(), x: 2.0, y: baseline, commands: 0, failed: false };
    let mut x = 2.0f32;
    let outlines = font.outline_glyphs();
    let mut glyph_count = 0usize;
    for run in runs.into_iter().take(80) {
        let text = label.get(run.clone()).ok_or("invalid text run")?;
        let mut buffer = harfrust::UnicodeBuffer::new();
        for (offset, ch) in text.char_indices() {
            buffer.add(ch, u32::try_from(offset).map_err(|_| "text offset overflow")?);
        }
        buffer.set_direction(if levels.get(run.start).is_some_and(|level| level.is_rtl()) {
            harfrust::Direction::RightToLeft
        } else {
            harfrust::Direction::LeftToRight
        });
        buffer.guess_segment_properties();
        let shaped = shaper.shape(buffer, harfrust::ShapeOptions::new());
        if shaped.glyph_infos().len() > 1024 {
            return Err("too many preview glyphs");
        }
        for (info, position) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            glyph_count = glyph_count.saturating_add(1);
            if glyph_count > 1024 {
                return Err("too many preview glyphs");
            }
            if info.glyph_id == 0 {
                return Err("face does not cover its name");
            }
            pen.x = x + position.x_offset as f32 * k;
            pen.y = baseline - position.y_offset as f32 * k;
            if !pen.x.is_finite() || !pen.y.is_finite() {
                return Err("invalid glyph position");
            }
            if let Some(outline) = outlines.get(GlyphId::new(info.glyph_id)) {
                outline.draw(DrawSettings::unhinted(Size::new(px), LocationRef::default()), &mut pen).map_err(|_| "invalid glyph outline")?;
            }
            if pen.failed {
                return Err("font outline exceeds preview budget");
            }
            x += position.x_advance as f32 * k;
            if !x.is_finite() {
                return Err("invalid glyph advance");
            }
            if x > width as f32 + px {
                break;
            }
        }
    }
    pen.path.close();
    let mut mask = Mask::new(width, height);
    crate::raster::fill_into(&pen.path, &mut mask, 0.0, 0.0);
    if !mask.a.iter().any(|value| value.is_finite() && *value > 0.0) {
        return Err("font name has no visible outline in this preview");
    }
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn real_distinct_typefaces_render_without_global_layout_or_face_access() {
        let a = face::extract(&mut Cursor::new(crate::fonts::INTER_REGULAR), 0).unwrap();
        let b = face::extract(&mut Cursor::new(crate::fonts::JETBRAINS_MONO_REGULAR), 0).unwrap();
        let a = raster(&a, "FilmCraft Aa Mm", 16.0, 240, 24).unwrap();
        let b = raster(&b, "FilmCraft Aa Mm", 16.0, 240, 24).unwrap();
        assert_ne!(a.a, b.a);
        assert!(a.a.iter().any(|v| *v > 0.0));
        assert!(b.a.iter().any(|v| *v > 0.0));
        assert_eq!(a.a.len(), 240 * 24);
    }
    #[test]
    fn real_font_mutations_do_not_panic_in_the_composed_preview_path() {
        let bytes = crate::fonts::INTER_REGULAR;
        let stride = (bytes.len() / 64).max(1);
        for end in (0..bytes.len()).step_by(stride) {
            let _ = std::panic::catch_unwind(|| preview(&mut Cursor::new(&bytes[..end]), 0, "FilmCraft Aa", 16.0, 240, 24)).unwrap();
        }
        for offset in (0..bytes.len()).step_by(stride) {
            let mut changed = bytes.to_vec();
            changed[offset] ^= 0x80;
            let _ = std::panic::catch_unwind(|| preview(&mut Cursor::new(changed), 0, "FilmCraft Aa", 16.0, 240, 24)).unwrap();
        }
    }
    #[test]
    fn public_family_preview_rejects_bad_requests_and_missing_families() {
        for (family, px, width, height) in
            [("", 16.0, 240, 24), ("Inter", f32::NAN, 240, 24), ("Inter", 16.0, usize::MAX, 24), ("Inter", 16.0, 240, usize::MAX)]
        {
            assert_eq!(super::family(family, px, width, height).unwrap_err(), PreviewError::InvalidInput);
        }
        assert_eq!(super::family("A missing FilmCraft preview family", 16.0, 240, 24).unwrap_err(), PreviewError::MissingFamily);
        assert!(super::family("Inter", 16.0, 240, 24).is_ok());
    }
    #[test]
    fn malformed_font_and_hostile_preview_sizes_fail_without_panics() {
        for font in [&[][..], &[0; 12][..], &[255; 512][..]] {
            assert!(std::panic::catch_unwind(|| raster(font, "Font", 16.0, 240, 24)).unwrap().is_err());
        }
        let bytes = crate::fonts::INTER_REGULAR;
        assert!(raster(bytes, "   ", 16.0, 240, 24).is_err());
        for (px, w, h) in [(f32::NAN, 240, 24), (f32::INFINITY, 240, 24), (16.0, usize::MAX, 24), (16.0, 240, usize::MAX), (0.0, 240, 24)] {
            assert!(raster(bytes, "Font", px, w, h).is_err());
        }
        assert!(raster(bytes, &"x".repeat(513), 16.0, 240, 24).is_err());
    }
    #[test]
    fn optional_font_preview_shapes_supported_scripts() {
        let cases = [
            ("BIZ UDPGothic", "日本語"),
            ("Noto Sans CJK SC", "你好"),
            ("NanumGothic", "한글"),
            ("Noto Sans Arabic", "مرحبا"),
            ("Noto Sans Hebrew", "שלום"),
            ("Noto Sans Devanagari", "नमस्ते"),
            ("Noto Sans Thai", "ภาษาไทย"),
            ("Noto Sans Georgian", "ქართული"),
        ];
        let mut checked = 0usize;
        for (family, label) in cases {
            let Some(font) = crate::fonts::CRAFT_FONTS.iter().find(|font| font.family == family && font.style == "Regular") else {
                continue;
            };
            let mask = preview(&mut Cursor::new(font.bytes), 0, label, 32.0, 400, 48).unwrap_or_else(|error| panic!("{family}: {error}"));
            assert!(mask.a.iter().any(|value| *value > 0.0), "{family}");
            assert_eq!(mask.a.len(), 400 * 48);
            checked += 1;
            eprintln!("temporary preview checked: {family}");
        }
        eprintln!("Optional preview script cases checked: {checked}; embedded font files: {}", crate::fonts::CRAFT_FONTS.len());
    }
}
