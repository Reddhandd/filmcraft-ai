//! Pinned local OCR models. No provider requests, paths, or dynamically selected model code.
use sha2::{Digest, Sha256};
pub(crate) const DETECTION_BYTES: usize = 2_499_479;
pub(crate) const RECOGNITION_BYTES: usize = 9_713_177;
const DETECTION_HASH: &str = "a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe";
const RECOGNITION_HASH: &str = "86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5";
pub(super) struct Engine(ocrs::OcrEngine);
fn verify(bytes: &[u8], count: usize, expected: &str) -> Result<(), String> {
    if bytes.len() != count || Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>() != expected {
        return Err("Local OCR model failed its pinned size or SHA256 check; reinstall the verified model files".into());
    }
    Ok(())
}
pub(crate) fn verify_models(detection: &[u8], recognition: &[u8]) -> Result<(), String> {
    verify(detection, DETECTION_BYTES, DETECTION_HASH)?;
    verify(recognition, RECOGNITION_BYTES, RECOGNITION_HASH)
}
impl Engine {
    pub(super) fn new(detection: Vec<u8>, recognition: Vec<u8>) -> Result<Self, String> {
        verify_models(&detection, &recognition)?;
        let detection = rten::Model::load(detection).map_err(|error| format!("Cannot load verified OCR detector: {error}"))?;
        let recognition = rten::Model::load(recognition).map_err(|error| format!("Cannot load verified OCR recognizer: {error}"))?;
        ocrs::OcrEngine::new(ocrs::OcrEngineParams { detection_model: Some(detection), recognition_model: Some(recognition), ..Default::default() })
            .map(Self)
            .map_err(|error| format!("Cannot initialize local OCR: {error}"))
    }
    pub(super) fn text(&self, raster: &super::visual::Raster) -> Result<String, String> {
        let pixels = u64::from(raster.width)
            .checked_mul(u64::from(raster.height))
            .filter(|pixels| *pixels > 0 && *pixels <= 1600 * 1600)
            .ok_or("OCR raster dimensions exceed their budget")?;
        if pixels.checked_mul(4) != Some(raster.rgba.len() as u64) {
            return Err("OCR pixel dimensions disagree with its input buffer".into());
        }
        let source = ocrs::ImageSource::from_bytes(&raster.rgba, (raster.width, raster.height)).map_err(|error| error.to_string())?;
        let input = self.0.prepare_input(source).map_err(|error| error.to_string())?;
        let text = self.0.get_text(&input).map_err(|error| format!("Local OCR failed: {error}"))?;
        if text.len() > super::MAX_PAGE_TEXT {
            return Err("Recognized page exceeds its 64 KiB text budget".into());
        }
        Ok(text)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unpinned_models_are_rejected_before_parsing() {
        assert!(Engine::new(Vec::new(), Vec::new()).is_err());
        assert!(Engine::new(vec![0; DETECTION_BYTES], vec![0; RECOGNITION_BYTES]).is_err());
    }
}
