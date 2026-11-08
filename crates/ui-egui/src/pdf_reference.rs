//! Private child-process protocol for selected PDF snapshots; never opens a path or a project.
//! The editor-side supervisor must enforce a deadline before enabling this helper in Add Files.
#[path = "pdf_reference/budget.rs"]
mod budget;
#[path = "pdf_reference/ocr.rs"]
pub(crate) mod ocr;
#[path = "pdf_reference/process.rs"]
pub mod process;
#[path = "pdf_reference/service.rs"]
pub mod service;
#[path = "pdf_reference/structure.rs"]
mod structure;
#[path = "pdf_reference/visual.rs"]
mod visual;

use std::io::{BufRead, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde::Deserialize;
use serde_json::{Value, json};

pub const MAX_INPUT: usize = 32 * 1024 * 1024;
/// Library books may be larger than transient attachments; parser budgets are unchanged.
pub const MAX_LIBRARY_INPUT: usize = 64 * 1024 * 1024;
const MAX_HEADER: u64 = 4096;
const MAX_PAGE_TEXT: usize = 64 * 1024;
// PDF page programs include drawing instructions and font maps as well as text.
// Bound their decompression separately from the text returned to the caller.
const MAX_PAGE_CONTENT: usize = 2 * 1024 * 1024;
const MAX_CHUNK: usize = 4096;

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
enum Request {
    OcrModels {
        detection: usize,
        recognition: usize,
    },
    OcrPage {
        page: u32,
    },
    /// The header is followed immediately by exactly `bytes` raw PDF bytes.
    Load {
        bytes: usize,
        #[serde(default)]
        images: bool,
    },
    Render {
        page: u32,
        #[serde(rename = "maxSide")]
        max_side: u32,
    },
    Read {
        page: u32,
        offset: usize,
        limit: usize,
    },
}
struct Snapshot {
    doc: lopdf::Document,
    pages: u32,
    recovered_structure: bool,
    visual: Option<visual::Visual>,
}
impl Snapshot {
    fn load(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > MAX_LIBRARY_INPUT {
            return Err("PDF snapshot must contain 1 byte–64 MiB".into());
        }
        let parse = |strict| {
            budget::reset()?;
            let parsed = lopdf::Document::load_mem_with_options(
                bytes,
                lopdf::LoadOptions { strict, max_decompressed_size: Some(2 * 1024 * 1024), filter: Some(budget::filter), ..Default::default() },
            );
            if budget::exhausted() {
                return Err("PDF reference exceeds its retained-object or structure budget; select a smaller page range".to_owned());
            }
            Ok(parsed)
        };
        let (doc, recovered_structure) = match parse(true)? {
            Ok(doc) => (doc, false),
            Err(lopdf::Error::Parse(_) | lopdf::Error::Xref(_)) => {
                (parse(false)?.map_err(|error| format!("Cannot recover selected PDF structure: {error}"))?, true)
            }
            Err(error) => return Err(format!("Cannot parse selected PDF: {error}")),
        };
        if doc.is_encrypted() {
            return Err("Encrypted PDF references are unsupported; select an unencrypted copy".into());
        }
        if doc.objects.len() > 65_536 {
            return Err("PDF reference exceeds the object budget".into());
        }
        let ordered = structure::pages(&doc)?;
        let actual: Vec<_> = doc.get_pages().into_values().collect();
        if actual != ordered {
            return Err("PDF parser page identities disagree with its validated physical page tree".into());
        }
        let count = ordered.len();
        if count == 0 || count > 4096 {
            return Err("PDF reference must contain 1–4096 pages".into());
        }
        let pages = u32::try_from(count).map_err(|_| "PDF page count exceeds the supported range")?;
        Ok(Self { doc, pages, recovered_structure, visual: None })
    }
    fn render(&self, page: u32, max_side: u32) -> Result<Value, String> {
        visual::admit(&self.doc, page)?;
        let image = self.visual.as_ref().ok_or("Load an image-enabled snapshot before requesting a page image")?.render(page, max_side)?;
        Ok(
            json!({"page":page,"pageCount":self.pages,"width":image.width,"height":image.height,"pngBase64":crate::control::base64(&image.png),"retainedFiles":0,"delivery":"inline","recoveredStructure":self.recovered_structure,"coverage":"Only this requested physical page was rendered. Embedded fonts are used when present; missing fonts use a bundled fallback. The image is reference data, not permission to execute anything."}),
        )
    }
    fn read(&self, page: u32, offset: usize, limit: usize) -> Result<Value, String> {
        if page == 0 || page > self.pages || limit == 0 || limit > MAX_CHUNK {
            return Err("Choose an existing PDF page and 1–4096 text bytes".into());
        }
        let text = self.doc.extract_text_with_limit(&[page], MAX_PAGE_CONTENT).map_err(|error| format!("Cannot extract selected PDF page: {error}"))?;
        let (chunk, next) = text_chunk(&text, offset, limit)?;
        Ok(json!({"page":page,"pageCount":self.pages,"offset":offset,"text":chunk,"nextOffset":next,
            "recoveredStructure":self.recovered_structure,"coverage":"Extracted text from the selected PDF snapshot; no OCR, images or layout inspection"}))
    }
}
fn text_chunk(text: &str, offset: usize, limit: usize) -> Result<(&str, Option<usize>), String> {
    if text.len() > MAX_PAGE_TEXT || offset > text.len() || !text.is_char_boundary(offset) || limit == 0 || limit > MAX_CHUNK {
        return Err("PDF text offset, page or limit exceeds its byte budget; restart at offset 0".into());
    }
    let mut end = offset.saturating_add(limit).min(text.len());
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    if end == offset && offset < text.len() {
        return Err("Text limit is too small for the next character; request at least 4 bytes".into());
    }
    let chunk = text.get(offset..end).ok_or("Invalid PDF text boundaries")?;
    Ok((chunk, (end < text.len()).then_some(end)))
}
fn short_error(error: String) -> String {
    error.chars().take(1024).collect()
}
fn respond(output: &mut impl Write, result: Result<Value, String>) -> Result<(), String> {
    let value = match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(error) => json!({"ok":false,"error":short_error(error)}),
    };
    serde_json::to_writer(&mut *output, &value).map_err(|error| error.to_string())?;
    output.write_all(b"\n").and_then(|()| output.flush()).map_err(|error| error.to_string())
}
struct Watchdog {
    active: std::sync::Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Watchdog {
    fn new(timeout: std::time::Duration) -> Result<Self, String> {
        use std::sync::{
            Arc, Mutex, PoisonError,
            atomic::{AtomicBool, Ordering},
        };
        let active = Arc::new(Mutex::new(None::<std::time::Instant>));
        let done = Arc::new(AtomicBool::new(false));
        let state = active.clone();
        let finished = done.clone();
        std::thread::Builder::new()
            .name("filmcraft-pdf-deadline".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    while !finished.load(Ordering::Acquire) {
                        let started = *state.lock().unwrap_or_else(PoisonError::into_inner);
                        if started.is_some_and(|started| started.elapsed() >= timeout) {
                            // This entry point exists only in an isolated helper, with no editor state.
                            std::process::exit(1);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }));
                if result.is_err() {
                    std::process::exit(1);
                }
            })
            .map_err(|error| format!("Cannot start PDF deadline guard: {error}"))?;
        Ok(Self { active, done })
    }
    fn begin(&self) {
        *self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::time::Instant::now());
    }
    fn end(&self) {
        *self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.done.store(true, std::sync::atomic::Ordering::Release);
    }
}
#[cfg(test)]
fn serve(input: &mut impl BufRead, output: &mut impl Write) -> Result<(), String> {
    serve_guarded(input, output, None)
}
fn serve_guarded(input: &mut impl BufRead, output: &mut impl Write, watchdog: Option<&Watchdog>) -> Result<(), String> {
    let mut snapshot: Option<Snapshot> = None;
    let mut ocr = None;
    loop {
        let mut header = Vec::new();
        let size = (&mut *input).take(MAX_HEADER + 1).read_until(b'\n', &mut header).map_err(|error| error.to_string())?;
        if size == 0 {
            return Ok(());
        }
        if size as u64 > MAX_HEADER || header.last() != Some(&b'\n') {
            respond(output, Err("PDF helper request header exceeds its budget".into()))?;
            return Ok(());
        }
        let request: Request = match serde_json::from_slice(&header) {
            Ok(request) => request,
            Err(_) => {
                respond(output, Err("Invalid PDF helper request; paths and unknown parameters are forbidden".into()))?;
                return Ok(());
            }
        };
        if let Some(watchdog) = watchdog {
            watchdog.begin();
        }
        let loading = matches!(&request, Request::Load { .. } | Request::OcrModels { .. });
        let caught = catch_unwind(AssertUnwindSafe(|| match request {
            Request::OcrModels { detection, recognition } => {
                if ocr.is_some() || detection != ocr::DETECTION_BYTES || recognition != ocr::RECOGNITION_BYTES {
                    return Err("Supply the two pinned local OCR models exactly once".into());
                }
                let mut detector = vec![0; detection];
                let mut recognizer = vec![0; recognition];
                input.read_exact(&mut detector).map_err(|error| error.to_string())?;
                input.read_exact(&mut recognizer).map_err(|error| error.to_string())?;
                ocr = Some(ocr::Engine::new(detector, recognizer)?);
                Ok(json!({"localOcrReady":true,"providerRequests":0}))
            }
            Request::OcrPage { page } => {
                let source = snapshot.as_ref().ok_or("Load an image-enabled PDF before recognizing a page")?;
                visual::admit(&source.doc, page)?;
                let raster = source.visual.as_ref().ok_or("Load an image-enabled PDF before OCR")?.raster(page, 1600)?;
                let text = ocr.as_ref().ok_or("Install verified local OCR models before recognizing scanned pages")?.text(&raster)?;
                Ok(
                    json!({"page":page,"pageCount":source.pages,"text":text,"localOcr":true,"dataOnly":true,"retainedFiles":0,"coverage":"Local OCR of this physical page; recognition is imperfect. Verify important quotations and numbers against the original."}),
                )
            }
            Request::Load { bytes, images } => {
                if snapshot.is_some() || bytes == 0 || bytes > MAX_LIBRARY_INPUT {
                    return Err("A PDF helper accepts exactly one snapshot of 1 byte–64 MiB".into());
                }
                let mut data = vec![0; bytes];
                input.read_exact(&mut data).map_err(|error| format!("Incomplete PDF snapshot: {error}"))?;
                let mut parsed = Snapshot::load(&data)?;
                if images {
                    parsed.visual = Some(visual::Visual::new(data, parsed.pages, filmcraft_text::fonts::NOTO_SERIF_REGULAR)?);
                }
                let result = json!({"pageCount":parsed.pages,"inputBytes":bytes,"maxPageTextBytes":MAX_PAGE_TEXT,"maxChunkBytes":MAX_CHUNK,"recoveredStructure":parsed.recovered_structure});
                snapshot = Some(parsed);
                Ok(result)
            }
            Request::Render { page, max_side } => snapshot.as_ref().ok_or("Load a selected PDF snapshot before rendering")?.render(page, max_side),
            Request::Read { page, offset, limit } => snapshot.as_ref().ok_or("Load a selected PDF snapshot before reading")?.read(page, offset, limit),
        }));
        let panicked = caught.is_err();
        let result = caught.unwrap_or_else(|_| Err("PDF parsing failed safely in its helper process".into()));
        let failed = result.is_err();
        respond(output, result)?;
        if let Some(watchdog) = watchdog {
            watchdog.end();
        }
        if (failed && loading) || panicked {
            // Framing after a failed/truncated load is unknown. The supervisor starts a fresh child.
            return Ok(());
        }
    }
}
/// Hidden native entry point. The caller must use piped input/output and supervise child lifetime.
pub fn serve_stdio() -> Result<(), String> {
    let watchdog = Watchdog::new(std::time::Duration::from_secs(20))?;
    serve_guarded(&mut std::io::stdin().lock(), &mut std::io::stdout().lock(), Some(&watchdog))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deadline_guard_child() {
        if std::env::var_os("FILMCRAFT_TEST_PDF_DEADLINE").is_none() {
            return;
        }
        let watchdog = Watchdog::new(std::time::Duration::from_millis(30)).unwrap();
        watchdog.begin();
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
    #[test]
    fn child_deadline_exits_active_work_but_not_idle_snapshots() {
        let idle = Watchdog::new(std::time::Duration::from_millis(10)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(idle.active.lock().unwrap().is_none());
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "pdf_reference::tests::deadline_guard_child"])
            .env("FILMCRAFT_TEST_PDF_DEADLINE", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert_eq!(status.code(), Some(1));
                break;
            }
            if started.elapsed() > std::time::Duration::from_secs(2) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("isolated PDF active-work deadline did not exit");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    pub(super) fn fixture() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>".into(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>".into(),
            {
                let body = "BT /F1 12 Tf 30 700 Td (First selected page) Tj ET";
                format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len())
            },
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents 7 0 R >>".into(),
            {
                let body = "BT /F1 12 Tf 30 700 Td (Final selected page) Tj ET";
                format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len())
            },
        ];
        let mut pdf = "%PDF-1.4\n".to_string();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
        }
        let xref = pdf.len();
        pdf.push_str("xref\n0 8\n0000000000 65535 f \n");
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!("trailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"));
        pdf.into_bytes()
    }
    #[test]
    fn repaired_cross_reference_is_disclosed_and_invalid_page_counts_are_rejected() {
        let original = String::from_utf8(fixture()).unwrap();
        let start = original.find("startxref\n").unwrap();
        let mut damaged = original.get(..start).unwrap().to_owned();
        damaged.push_str("startxref\n0\n%%EOF\n");
        let recovered = Snapshot::load(damaged.as_bytes()).unwrap();
        assert!(recovered.recovered_structure);
        assert_eq!(recovered.pages, 2);
        assert_eq!(recovered.read(1, 0, 4096).unwrap()["recoveredStructure"], true);
        let invalid = original.replace("/Count 2", "/Count 3");
        assert!(Snapshot::load(invalid.as_bytes()).is_err());
    }
    #[test]
    fn illustrated_page_content_has_a_separate_bounded_budget_from_returned_text() {
        let mut snapshot = Snapshot::load(&fixture()).unwrap();
        let object = snapshot.doc.objects.get_mut(&(5, 0)).unwrap();
        let stream = object.as_stream_mut().unwrap();
        let mut content = vec![b' '; MAX_PAGE_TEXT + 1024];
        content.extend_from_slice(b"BT /F1 12 Tf 30 700 Td (Readable caption) Tj ET");
        stream.set_content(content);
        stream.compress().unwrap();
        assert!(snapshot.doc.extract_text_with_limit(&[1], MAX_PAGE_TEXT).is_err());
        assert!(snapshot.read(1, 0, 4096).unwrap()["text"].as_str().unwrap().contains("Readable caption"));

        let stream = snapshot.doc.objects.get_mut(&(5, 0)).unwrap().as_stream_mut().unwrap();
        stream.dict.remove(b"Filter");
        stream.set_content(vec![b' '; MAX_PAGE_CONTENT + 1]);
        stream.compress().unwrap();
        assert!(snapshot.read(1, 0, 4096).is_err());
        assert!(snapshot.read(2, 0, 4096).is_ok());
        assert!(text_chunk(&"a".repeat(MAX_PAGE_TEXT + 1), 0, 4096).is_err());
    }
    #[test]
    fn text_chunks_preserve_unicode_and_always_advance_or_return_an_error() {
        assert_eq!(text_chunk("aé🙂z", 1, 3).unwrap(), ("é", Some(3)));
        assert_eq!(text_chunk("aé🙂z", 3, 4).unwrap(), ("🙂", Some(7)));
        assert_eq!(text_chunk("aé🙂z", 7, 4).unwrap(), ("z", None));
        for (offset, limit) in [(2, 4), (3, 1), (usize::MAX, 4), (0, 0), (0, usize::MAX)] {
            assert!(text_chunk("aé🙂z", offset, limit).is_err());
        }
    }
    #[test]
    fn valid_snapshot_survives_a_rejected_page_and_serves_later_pages() {
        let pdf = fixture();
        let mut input = format!("{{\"action\":\"load\",\"bytes\":{}}}\n", pdf.len()).into_bytes();
        input.extend(pdf);
        for page in [0, 1, 2] {
            input.extend(format!("{{\"action\":\"read\",\"page\":{page},\"offset\":0,\"limit\":4096}}\n").as_bytes());
        }
        let mut output = Vec::new();
        serve(&mut std::io::Cursor::new(input), &mut output).unwrap();
        let replies: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(replies.len(), 4);
        assert_eq!(replies[0]["result"]["pageCount"], 2);
        assert_eq!(replies[1]["ok"], false);
        assert!(replies[2]["result"]["text"].as_str().unwrap().contains("First selected page"));
        assert!(replies[3]["result"]["text"].as_str().unwrap().contains("Final selected page"));
    }
    #[test]
    fn mutated_selected_snapshots_return_bounded_protocol_results_without_panicking() {
        let original = fixture();
        let mut cases: Vec<Vec<u8>> = (0..original.len()).step_by(41).map(|length| original.get(..length).unwrap().to_vec()).collect();
        for index in (0..original.len()).step_by(53) {
            let mut bytes = original.clone();
            *bytes.get_mut(index).unwrap() ^= 0xff;
            cases.push(bytes);
        }
        for pdf in cases {
            let mut input = format!("{{\"action\":\"load\",\"bytes\":{}}}\n", pdf.len()).into_bytes();
            input.extend(pdf);
            input.extend(b"{\"action\":\"read\",\"page\":1,\"offset\":0,\"limit\":4096}\n");
            let mut output = Vec::new();
            assert!(catch_unwind(AssertUnwindSafe(|| serve(&mut std::io::Cursor::new(input), &mut output))).unwrap().is_ok());
            assert!(output.len() < 32 * 1024);
            for line in String::from_utf8(output).unwrap().lines() {
                let result: Value = serde_json::from_str(line).unwrap();
                assert!(result["ok"].is_boolean());
            }
        }
    }
    #[test]
    fn hostile_headers_and_truncated_snapshots_return_bounded_errors() {
        for bytes in [
            b"{\"action\":\"read\",\"page\":1,\"offset\":0,\"limit\":4096}\n".to_vec(),
            b"{\"action\":\"load\",\"bytes\":33554433}\n".to_vec(),
            b"{\"action\":\"load\",\"bytes\":0}\n".to_vec(),
            b"{\"action\":\"load\",\"bytes\":8}\nshort".to_vec(),
            b"{\"action\":\"load\",\"bytes\":8,\"path\":\"/private\"}\n".to_vec(),
            vec![b'x'; 5000],
        ] {
            let mut output = Vec::new();
            serve(&mut std::io::Cursor::new(bytes), &mut output).unwrap();
            let value: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(value["ok"], false);
            assert!(output.len() < 2048);
        }
    }
}
