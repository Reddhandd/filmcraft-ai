//! Bounded local model admission. The helper rechecks the same fingerprints after transport.
use crate::pdf_reference::ocr;
use std::path::Path;
pub(super) struct Models {
    pub detection: Vec<u8>,
    pub recognition: Vec<u8>,
}
pub(super) fn read(root: &Path) -> Result<Option<Models>, String> {
    let dir = root.join("OcrModels");
    match std::fs::symlink_metadata(&dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot inspect local OCR models: {error}")),
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => return Err("Local OCR models must use their own regular directory".into()),
        Ok(_) => {}
    }
    let detection = super::store::read(&dir.join("detection.onnx"), ocr::DETECTION_BYTES)?;
    let recognition = super::store::read(&dir.join("recognition.onnx"), ocr::RECOGNITION_BYTES)?;
    match (detection, recognition) {
        (None, None) => Ok(None),
        (Some(detection), Some(recognition)) => {
            ocr::verify_models(&detection, &recognition)?;
            Ok(Some(Models { detection, recognition }))
        }
        _ => Err("Local OCR model installation is incomplete; install both verified model files before retrying".into()),
    }
}
/// Native user action only. Books and project data are never included in these requests.
pub(super) fn install(root: &Path) -> Result<serde_json::Value, String> {
    use std::{io::Read, time::Duration};
    let dir = root.join("OcrModels");
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("Cannot create local model directory: {error}")),
    }
    let metadata = std::fs::symlink_metadata(&dir).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("OCR model destination must be a regular private directory".into());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::Rustls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .unversioned_rustls_crypto_provider(std::sync::Arc::new(rustls_rustcrypto::provider()))
                .build(),
        )
        .timeout_global(Some(Duration::from_secs(45)))
        .max_redirects(0)
        .build()
        .into();
    let fetch = |name: &str, size: usize| -> Result<Vec<u8>, String> {
        let url = format!("https://ocrs-models.s3-accelerate.amazonaws.com/text-{name}.onnx");
        let mut response = agent.get(&url).call().map_err(|error| format!("Cannot download public OCR model: {error}"))?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size.saturating_add(1)).map_err(|error| error.to_string())?;
        response.body_mut().as_reader().take(size.saturating_add(1) as u64).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
        if bytes.len() != size {
            return Err("Downloaded OCR model has an unexpected size; nothing was installed".into());
        }
        Ok(bytes)
    };
    let detection = fetch("detection", ocr::DETECTION_BYTES)?;
    let recognition = fetch("recognition", ocr::RECOGNITION_BYTES)?;
    ocr::verify_models(&detection, &recognition)?;
    let write = |name: &str, data: &[u8]| {
        filmcraft_format::atomic::atomic_write(&dir.join(name), data).map_err(|error| format!("Cannot install verified local OCR models: {error}"))
    };
    write("LICENSE-CC-BY-SA-4.0.txt", include_str!("../../../../../docs/licenses/ocrs-models-CC-BY-SA-4.0.txt").as_bytes())?;
    for name in ["detection", "recognition"] {
        let sidecar = format!(
            "asset:        {name}.onnx\ntitle:        Ocrs text {name} model\nauthor:       Robert Knight\nsource:       https://ocrs-models.s3-accelerate.amazonaws.com/text-{name}.onnx\nlicense:      CC-BY-SA-4.0\nlicense-file: LICENSE-CC-BY-SA-4.0.txt\nadded:        {} by FilmCraft user\nnotes:        Unmodified, checksum-verified download. Author license clarification: https://github.com/robertknight/ocrs-models/issues/34\n",
            chrono::Utc::now().date_naive()
        );
        write(&format!("{name}.onnx.attribution"), sidecar.as_bytes())?;
    }
    write("detection.onnx", &detection)?;
    write("recognition.onnx", &recognition)?;
    read(root)?.ok_or("Verified model installation is incomplete")?;
    Ok(
        serde_json::json!({"installed":true,"bytes":detection.len().saturating_add(recognition.len()),"localOnly":true,"providerRequests":0,"license":"CC-BY-SA-4.0"}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_incomplete_corrupt_and_linked_model_stores_fail_safely() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path()).unwrap().is_none());
        let models = dir.path().join("OcrModels");
        std::fs::create_dir(&models).unwrap();
        std::fs::write(models.join("detection.onnx"), b"not an admitted model").unwrap();
        assert!(read(dir.path()).is_err());
        std::fs::write(models.join("recognition.onnx"), b"also corrupt").unwrap();
        assert!(read(dir.path()).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_dir_all(&models).unwrap();
            std::os::unix::fs::symlink(dir.path(), &models).unwrap();
            assert!(read(dir.path()).is_err());
        }
    }
}
