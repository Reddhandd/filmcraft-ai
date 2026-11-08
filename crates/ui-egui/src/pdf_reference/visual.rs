//! On-demand page rendering inside the isolated PDF helper; no screenshot files.
use hayro::{
    PixmapSettings, RenderCache, RenderSettings,
    hayro_interpret::{InterpreterSettings, font::FontData},
    hayro_syntax::Pdf,
};
use lopdf::{Document, Object};
use std::{collections::BTreeSet, sync::Arc};
const MAX_PIXELS: usize = 1600 * 1600;
const MAX_PNG: usize = 256 * 1024;
pub(super) struct Visual {
    pdf: Pdf,
    settings: InterpreterSettings,
}
pub(super) struct Raster {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
pub(super) struct Image {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
fn resolved<'a>(doc: &'a Document, mut object: &'a Object) -> Result<&'a Object, String> {
    let mut seen = BTreeSet::new();
    for _ in 0..32 {
        let Object::Reference(id) = object else { return Ok(object) };
        if !seen.insert(*id) {
            return Err("Cyclic PDF resource reference".into());
        }
        object = doc.objects.get(id).ok_or("PDF resource reference is missing")?;
    }
    Err("PDF resource reference exceeds its depth budget".into())
}
fn program(bytes: &[u8], total: &mut usize) -> Result<(), String> {
    *total = total.checked_add(bytes.len()).filter(|size| *size <= 8 * 1024 * 1024).ok_or("Page programs exceed the 8 MiB render budget")?;
    let content = lopdf::content::Content::decode_strict(bytes).map_err(|error| format!("Cannot validate page rendering instructions: {error}"))?;
    if content.operations.len() > 65_536 || content.operations.iter().any(|operation| operation.operator == "BI") {
        return Err("Inline images or oversized page programs are not supported by bounded page rendering".into());
    }
    Ok(())
}
/// Resolve only this page's resource graph. Pixel admission precedes image decoding.
pub(super) fn admit(doc: &Document, page: u32) -> Result<(), String> {
    let pages = super::structure::pages(doc)?;
    let index = page.checked_sub(1).and_then(|page| usize::try_from(page).ok()).ok_or("Choose a positive page")?;
    let id = *pages.get(index).ok_or("Choose an existing PDF page")?;
    let (direct, refs) = doc.get_page_resources(id).map_err(|error| error.to_string())?;
    let mut programs = 0usize;
    program(&doc.get_page_content_with_limit(id, 2 * 1024 * 1024).map_err(|error| error.to_string())?, &mut programs)?;
    let mut stack = Vec::new();
    if let Some(dict) = direct {
        if dict.len() > 32_768 {
            return Err("Page resource dictionary exceeds its budget".into());
        }
        stack.extend(dict.iter().map(|(_, value)| (value, 0usize)));
    }
    for id in refs {
        if stack.len() >= 32_768 {
            return Err("Page resource roots exceed their budget".into());
        }
        stack.push((doc.objects.get(&id).ok_or("Page resources are missing")?, 0));
    }
    let mut seen = BTreeSet::new();
    let mut nodes = 0usize;
    let mut pixels = 0usize;
    while let Some((object, depth)) = stack.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > 32_768 || depth > 64 || stack.len() > 32_768 {
            return Err("Page resource graph exceeds its render budget".into());
        }
        match object {
            Object::Reference(id) => {
                if seen.insert(*id) {
                    stack.push((doc.objects.get(id).ok_or("Page resource is missing")?, depth.saturating_add(1)));
                }
            }
            Object::Array(values) => {
                if stack.len().saturating_add(values.len()) > 32_768 {
                    return Err("Page resource array exceeds its budget".into());
                }
                stack.extend(values.iter().map(|value| (value, depth.saturating_add(1))));
            }
            Object::Dictionary(dict) => {
                if stack.len().saturating_add(dict.len()) > 32_768 {
                    return Err("Page resource dictionary exceeds its budget".into());
                }
                stack.extend(
                    dict.iter()
                        .filter(|(key, _)| !matches!(key.as_slice(), b"Parent" | b"P" | b"Pages" | b"Root"))
                        .map(|(_, value)| (value, depth.saturating_add(1))),
                );
            }
            Object::Stream(stream) => {
                let kind = match stream.dict.get(b"Subtype") {
                    Ok(object) => resolved(doc, object)?.as_name().map_err(|error| error.to_string())?,
                    Err(_) => b"",
                };
                if kind != b"Image" && (stream.dict.has(b"Width") || stream.dict.has(b"Height")) {
                    return Err("Unrecognized image resource cannot be rendered safely".into());
                }
                if kind == b"Image" {
                    let dimension = |key: &[u8]| -> Result<usize, String> {
                        let object = stream.dict.get(key).map_err(|error| error.to_string())?;
                        resolved(doc, object)?
                            .as_i64()
                            .ok()
                            .and_then(|n| usize::try_from(n).ok())
                            .filter(|n| (1..=16_384).contains(n))
                            .ok_or("Invalid or oversized PDF image dimensions".into())
                    };
                    let area = dimension(b"Width")?
                        .checked_mul(dimension(b"Height")?)
                        .filter(|n| *n <= 16 * 1024 * 1024)
                        .ok_or("PDF image exceeds 16 million pixels")?;
                    pixels = pixels.checked_add(area).filter(|n| *n <= 64 * 1024 * 1024).ok_or("Page resources exceed 64 million decoded image pixels")?;
                } else if kind == b"Form" || stream.dict.has(b"PatternType") {
                    program(&stream.get_plain_content_with_limit(2 * 1024 * 1024).map_err(|error| error.to_string())?, &mut programs)?;
                }
                if stack.len().saturating_add(stream.dict.len()) > 32_768 {
                    return Err("Page resource stream exceeds its budget".into());
                }
                stack.extend(
                    stream
                        .dict
                        .iter()
                        .filter(|(key, _)| !matches!(key.as_slice(), b"Parent" | b"P" | b"Pages" | b"Root"))
                        .map(|(_, value)| (value, depth.saturating_add(1))),
                );
            }
            _ => {}
        }
    }
    Ok(())
}
impl Visual {
    pub(super) fn new(bytes: Vec<u8>, pages: u32, fallback: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > super::MAX_LIBRARY_INPUT || fallback.is_empty() || fallback.len() > 2 * 1024 * 1024 {
            return Err("Visual PDF snapshot exceeds its input budget".into());
        }
        let pdf = Pdf::new(bytes).map_err(|error| format!("Cannot render selected PDF: {error:?}"))?;
        if pdf.pages().len() != pages as usize {
            return Err("Independent PDF parsers disagree about physical page count".into());
        }
        let font: FontData = Arc::new(fallback.to_vec());
        let settings = InterpreterSettings { font_resolver: Arc::new(move |_| Some((font.clone(), 0))), ..Default::default() };
        Ok(Self { pdf, settings })
    }
    /// Fixed-resolution pixels for local OCR; never downscale silently to fit a network payload.
    pub(super) fn raster(&self, page: u32, max_side: u32) -> Result<Raster, String> {
        if !(256..=1600).contains(&max_side) {
            return Err("Page raster size must be 256–1600 pixels".into());
        }
        let index = page.checked_sub(1).and_then(|page| usize::try_from(page).ok()).ok_or("Choose a positive PDF page")?;
        let page = self.pdf.pages().get(index).ok_or("Choose an existing physical PDF page")?;
        let (width, height) = page.render_dimensions();
        if !width.is_finite() || !height.is_finite() || !(1.0..=100_000.0).contains(&width) || !(1.0..=100_000.0).contains(&height) {
            return Err("PDF page has invalid or oversized geometry".into());
        }
        let scale = max_side as f32 / width.max(height);
        if width * scale < 1.0 || height * scale < 1.0 {
            return Err("PDF page aspect ratio is unsupported".into());
        }
        let pixmap = hayro::render(
            page,
            &RenderCache::new(),
            &self.settings,
            &RenderSettings::default(),
            &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: hayro::vello_cpu::color::palette::css::WHITE },
        );
        let width = u32::from(pixmap.width());
        let height = u32::from(pixmap.height());
        let pixels = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
            .filter(|pixels| *pixels > 0 && *pixels <= MAX_PIXELS)
            .ok_or("Rendered page exceeds its pixel budget")?;
        let bytes = pixels.checked_mul(4).ok_or("Rendered pixel byte count overflow")?;
        if pixmap.data_as_u8_slice().len() != bytes {
            return Err("Rendered pixel dimensions disagree with its buffer".into());
        }
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(bytes).map_err(|error| format!("Cannot allocate bounded page pixels: {error}"))?;
        rgba.extend_from_slice(pixmap.data_as_u8_slice());
        Ok(Raster { rgba, width, height })
    }
    pub(super) fn render(&self, page: u32, max_side: u32) -> Result<Image, String> {
        use image::ImageEncoder;
        if !(256..=1600).contains(&max_side) {
            return Err("Page image size must be 256–1600 pixels".into());
        }
        let mut side = max_side;
        for _ in 0..8 {
            let raster = self.raster(page, side)?;
            let mut png = Vec::new();
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(&raster.rgba, raster.width, raster.height, image::ExtendedColorType::Rgba8)
                .map_err(|error| format!("Cannot encode page image: {error}"))?;
            if png.len() <= MAX_PNG {
                return Ok(Image { png, width: raster.width, height: raster.height });
            }
            if side <= 256 {
                break;
            }
            side = (side.saturating_mul(3) / 4).max(256);
        }
        Err("Page image exceeds its 256 KiB delivery budget even after bounded downscaling".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hostile_image_dimensions_are_rejected_before_any_decoding() {
        let bytes = super::super::tests::fixture();
        for width in [Object::Integer(-1), Object::Real(f32::NAN), Object::Integer(16_384)] {
            let mut doc = Document::load_mem(&bytes).unwrap();
            let mut image = lopdf::dictionary! {"Subtype"=>"Image","Width"=>width,"Height"=>16_384};
            image.set("Length", 0);
            doc.objects.insert((9, 0), Object::Stream(lopdf::Stream::new(image, Vec::new())));
            doc.get_dictionary_mut((4, 0)).unwrap().set("Resources", lopdf::dictionary! {"XObject"=>lopdf::dictionary!{"Photo"=>(9,0)}});
            assert!(admit(&doc, 1).is_err());
        }
    }
    #[test]
    fn requested_page_renders_without_files_and_rejects_hostile_parameters() {
        let bytes = super::super::tests::fixture();
        let doc = Document::load_mem(&bytes).unwrap();
        admit(&doc, 1).unwrap();
        let visual = Visual::new(bytes, 2, filmcraft_text::fonts::NOTO_SERIF_REGULAR).unwrap();
        let raster = visual.raster(1, 1600).unwrap();
        assert_eq!(raster.rgba.len(), raster.width as usize * raster.height as usize * 4);
        assert!((1599..=1600).contains(&raster.width.max(raster.height)));
        let image = visual.render(1, 800).unwrap();
        assert!(image.png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(image.png.len() <= MAX_PNG);
        assert!(image.width <= 800 && image.height <= 800);
        assert!(visual.render(0, 800).is_err());
        assert!(visual.render(3, 800).is_err());
        assert!(visual.render(1, u32::MAX).is_err());
    }
}
