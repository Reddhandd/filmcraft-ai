//! Unencrypted EPUB reading order. Imported prose and markup never execute or fetch resources.
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};
const MAX_ENTRIES: usize = 4096;
const MAX_ENTRY: u64 = 32 * 1024 * 1024;
const MAX_EXPANDED: u64 = 512 * 1024 * 1024;
const MAX_XML: usize = 4 * 1024 * 1024;
pub(super) struct Book {
    pub chapters: Vec<(String, String)>,
    pub illustrations: BTreeMap<String, Illustration>,
}
pub(super) struct Illustration {
    pub path: String,
    pub media: String,
    pub section: u32,
    pub chapter: String,
}
fn safe_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > 1024
        || path.starts_with('/')
        || path.contains(['\\', ':', '%', '?', '#'])
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| part == ".." || part == "." || part.is_empty())
    {
        return Err("EPUB contains an unsafe or unsupported resource path".into());
    }
    Ok(())
}
fn resolve(base: &str, href: &str) -> Result<String, String> {
    safe_path(base)?;
    if href.is_empty() || href.len() > 4096 {
        return Err("EPUB resource URI exceeds its byte budget".into());
    }
    let source = href.as_bytes();
    let mut decoded = Vec::new();
    let mut at = 0usize;
    while let Some(byte) = source.get(at) {
        if *byte == b'%' {
            let high = source.get(at.saturating_add(1)).and_then(|byte| char::from(*byte).to_digit(16)).ok_or("Invalid EPUB URI escape")?;
            let low = source.get(at.saturating_add(2)).and_then(|byte| char::from(*byte).to_digit(16)).ok_or("Invalid EPUB URI escape")?;
            decoded.push((high * 16 + low) as u8);
            at = at.saturating_add(3);
        } else {
            decoded.push(*byte);
            at = at.saturating_add(1);
        }
    }
    let decoded = std::str::from_utf8(&decoded).map_err(|_| "EPUB resource URI must decode to UTF-8")?;
    if decoded.starts_with('/') || decoded.contains(['\\', ':', '%', '?', '#']) || decoded.chars().any(char::is_control) {
        return Err("External or unsafe EPUB resource URI".into());
    }
    let mut components: Vec<&str> = base.rsplit_once('/').map(|(dir, _)| dir.split('/').collect()).unwrap_or_default();
    for component in decoded.split('/') {
        match component {
            "" => return Err("Empty EPUB resource path component".into()),
            "." => {}
            ".." => {
                components.pop().ok_or("EPUB resource escapes its archive root")?;
            }
            other => components.push(other),
        }
    }
    let path = components.join("/");
    safe_path(&path)?;
    Ok(path)
}

fn xml(text: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: 200_000, ..Default::default() })
        .map_err(|error| format!("Cannot parse EPUB XML (DTDs and external entities are unsupported): {error}"))
}
fn read(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, path: &str, limit: usize) -> Result<String, String> {
    safe_path(path)?;
    let file = archive.by_name(path).map_err(|error| format!("Missing EPUB resource {path}: {error}"))?;
    if file.encrypted() || file.is_symlink() || file.size() > limit as u64 {
        return Err("EPUB resource is encrypted, linked or oversized".into());
    }
    let mut text = String::new();
    file.take(limit.saturating_add(1) as u64).read_to_string(&mut text).map_err(|error| format!("Cannot read EPUB text: {error}"))?;
    if text.len() > limit {
        return Err("EPUB text exceeds its expansion budget".into());
    }
    Ok(text)
}
pub(super) fn parse(bytes: &[u8]) -> Result<Book, String> {
    if bytes.is_empty() || bytes.len() > 256 * 1024 * 1024 {
        return Err("EPUB must contain 1 byte–256 MiB".into());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| format!("Cannot open EPUB: {error}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err("EPUB exceeds 4096 archive entries".into());
    }
    let mut names = BTreeSet::new();
    let mut expanded = 0u64;
    for n in 0..archive.len() {
        let file = archive.by_index(n).map_err(|error| format!("Cannot inspect EPUB entry: {error}"))?;
        let name = file.name().trim_end_matches('/');
        safe_path(name)?;
        if !names.insert(name.to_owned()) || file.encrypted() || file.is_symlink() || file.size() > MAX_ENTRY {
            return Err("EPUB contains duplicate, encrypted, linked or oversized entries".into());
        }
        expanded = expanded.checked_add(file.size()).filter(|total| *total <= MAX_EXPANDED).ok_or("EPUB exceeds its total inflation budget")?;
    }
    // Font obfuscation is common even in otherwise unencrypted EPUBs. We do not
    // decode or use those fonts. Protected prose/images and all DRM remain unsupported.
    if names.contains("META-INF/rights.xml") {
        return Err("DRM-protected EPUBs are unsupported; select an authorized unencrypted copy".into());
    }
    let mut obfuscated_fonts = BTreeSet::new();
    if names.contains("META-INF/encryption.xml") {
        let text = read(&mut archive, "META-INF/encryption.xml", MAX_XML)?;
        let encryption = xml(&text)?;
        for node in encryption.descendants().filter(|node| node.has_tag_name("EncryptedData")) {
            let methods: Vec<_> = node.descendants().filter(|child| child.has_tag_name("EncryptionMethod")).collect();
            let references: Vec<_> = node.descendants().filter(|child| child.has_tag_name("CipherReference")).collect();
            if methods.len() != 1
                || references.len() != 1
                || methods.first().and_then(|method| method.attribute("Algorithm")) != Some("http://www.idpf.org/2008/embedding")
            {
                return Err("Protected EPUB content is unsupported; select an authorized unencrypted copy".into());
            }
            let path = references.first().and_then(|reference| reference.attribute("URI")).ok_or("Protected EPUB resource identity is missing")?;
            safe_path(path)?;
            obfuscated_fonts.insert(path.to_owned());
        }
        if obfuscated_fonts.is_empty() {
            return Err("Unrecognized EPUB encryption metadata".into());
        }
    }
    if read(&mut archive, "mimetype", 128)?.trim() != "application/epub+zip" {
        return Err("Selected ZIP is not an EPUB".into());
    }
    let container_text = read(&mut archive, "META-INF/container.xml", MAX_XML)?;
    let container = xml(&container_text)?;
    let packages: Vec<_> = container.descendants().filter(|node| node.has_tag_name("rootfile")).collect();
    if packages.len() != 1 {
        return Err("EPUB requires exactly one supported package root".into());
    }
    let package_path = packages.first().and_then(|node| node.attribute("full-path")).ok_or("EPUB package root is missing")?;
    let package_text = read(&mut archive, package_path, MAX_XML)?;
    let package = xml(&package_text)?;
    let mut manifest = BTreeMap::new();
    for node in package.descendants().filter(|node| node.has_tag_name("item")) {
        let id = node.attribute("id").filter(|id| !id.is_empty() && id.len() <= 256).ok_or("Invalid EPUB manifest identity")?;
        let href = node.attribute("href").ok_or("EPUB manifest resource path is missing")?;
        let path = resolve(package_path, href)?;
        let media = node.attribute("media-type").unwrap_or("");
        if manifest.insert(id, (path, media)).is_some() {
            return Err("EPUB repeats a manifest identity".into());
        }
    }
    for font in &obfuscated_fonts {
        if !manifest.values().any(|(path, media)| {
            path == font && matches!(*media, "font/otf" | "font/ttf" | "application/vnd.ms-opentype" | "application/font-sfnt" | "application/x-font-ttf")
        }) {
            return Err("EPUB encryption metadata references protected content other than an unused font".into());
        }
    }
    let mut chapters = Vec::new();
    let mut illustrations = BTreeMap::new();
    let mut total = 0usize;
    let mut seen = BTreeSet::new();
    for node in package.descendants().filter(|node| node.has_tag_name("itemref")) {
        if node.attribute("linear") == Some("no") {
            continue;
        }
        let id = node.attribute("idref").ok_or("EPUB spine identity missing")?;
        let (path, media) = manifest.get(id).ok_or("EPUB spine references an unknown resource")?;
        if *media != "application/xhtml+xml" || !seen.insert(path.clone()) {
            return Err("EPUB spine must contain distinct XHTML sections".into());
        }
        let text = read(&mut archive, path, MAX_XML)?;
        let chapter = xml(&text)?;
        let title = chapter.descendants().find(|node| node.has_tag_name("title")).and_then(|node| node.text()).unwrap_or(path).trim();
        let title: String = title.chars().take(256).collect();
        let body = chapter.descendants().find(|node| node.has_tag_name("body")).ok_or("EPUB section has no body")?;
        let mut prose = String::new();
        for node in body.descendants().filter(|node| node.is_text()) {
            if node.ancestors().any(|parent| matches!(parent.tag_name().name(), "script" | "style" | "iframe" | "object" | "noscript")) {
                continue;
            }
            let chunk = node.text().unwrap_or("");
            if prose.len().saturating_add(chunk.len()).saturating_add(1) > 512 * 1024 {
                return Err("EPUB section exceeds 512 KiB; split the chapter".into());
            }
            prose.push_str(chunk);
            prose.push(' ');
        }
        let section = u32::try_from(chapters.len().saturating_add(1)).map_err(|_| "Invalid EPUB section ordinal")?;
        for (n, node) in body.descendants().filter(|node| node.has_tag_name("img")).enumerate() {
            if illustrations.len() >= 4096 {
                return Err("EPUB exceeds 4096 admitted illustration references".into());
            }
            let href = node.attribute("src").ok_or("EPUB illustration has no source identity")?;
            let image_path = resolve(path, href)?;
            let (_, media) = manifest.values().find(|(path, _)| path == &image_path).ok_or("EPUB illustration is not admitted by its manifest")?;
            if !matches!(*media, "image/png" | "image/jpeg" | "image/svg+xml") {
                return Err("Unsupported EPUB illustration media type".into());
            }
            let resource = format!("section-{section}-image-{}", n.saturating_add(1));
            let alt: String = node.attribute("alt").unwrap_or("Illustration").chars().take(256).collect();
            if prose.len().saturating_add(resource.len()).saturating_add(alt.len()).saturating_add(64) > 512 * 1024 {
                return Err("EPUB section and illustration references exceed 512 KiB".into());
            }
            prose.push_str(&format!("\n[Requested illustration resource: {resource}; {alt}]\n"));
            illustrations.insert(resource, Illustration { path: image_path, media: (*media).to_owned(), section, chapter: title.clone() });
        }
        total = total.saturating_add(prose.len());
        if total > super::model::MAX_TEXT {
            return Err("EPUB exceeds its extracted-text budget".into());
        }
        chapters.push((title, prose));
        if chapters.len() > 4096 {
            return Err("EPUB exceeds 4096 sections".into());
        }
    }
    if chapters.is_empty() {
        return Err("EPUB has no readable linear sections".into());
    }
    Ok(Book { chapters, illustrations })
}
/// Revalidate the book manifest and exact opaque resource identity before inflating one image.
pub(super) fn illustration(bytes: &[u8], resource: &str) -> Result<(Illustration, Vec<u8>), String> {
    if resource.is_empty() || resource.len() > 128 {
        return Err("Choose a bounded illustration identity from a cited section".into());
    }
    let mut book = parse(bytes)?;
    let admitted = book.illustrations.remove(resource).ok_or("Illustration identity is not present in this admitted EPUB")?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let file = archive.by_name(&admitted.path).map_err(|error| error.to_string())?;
    if file.encrypted() || file.is_symlink() || file.size() > MAX_ENTRY {
        return Err("EPUB illustration exceeds its admission budget".into());
    }
    let mut data = Vec::new();
    file.take(MAX_ENTRY.saturating_add(1)).read_to_end(&mut data).map_err(|error| error.to_string())?;
    if data.len() as u64 > MAX_ENTRY {
        return Err("EPUB illustration expanded beyond its byte budget".into());
    }
    Ok((admitted, data))
}
#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::Write;
    pub(crate) fn fixture(extra: Option<(&str, &str)>) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in [
            ("mimetype", "application/epub+zip"),
            ("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="OPS/book.opf"/></rootfiles></container>"#),
            ("OPS/book.opf", r#"<package><manifest><item id="c1" href="one.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#),
            ("OPS/one.xhtml", "<html><head><title>Band sync</title></head><body><p>Align performance to the song.</p><script>DO NOT EXECUTE</script></body></html>"),
        ].into_iter().chain(extra) {
            archive.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            archive.write_all(text.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }
    #[test]
    fn relative_uris_resolve_inside_archive_and_encoded_escape_is_rejected() {
        assert_eq!(resolve("OPS/Text/chapter.xhtml", "../Images/a%20diagram.png").unwrap(), "OPS/Images/a diagram.png");
        assert_eq!(resolve("OPS/book.opf", "./Text/chapter.xhtml").unwrap(), "OPS/Text/chapter.xhtml");
        for href in [
            "../../../outside",
            "%2fprivate/file",
            "https://example.com/book.png",
            "..%2f..%2f..%2foutside",
            "image%00.png",
            "a%5cb.png",
            "truncated%2",
            "double%252fencoded",
        ] {
            assert!(resolve("OPS/Text/chapter.xhtml", href).is_err(), "{href}");
        }
    }
    #[test]
    fn imports_chapter_citations_without_executing_markup() {
        let book = parse(&fixture(None)).unwrap();
        assert_eq!(book.chapters[0].0, "Band sync");
        assert!(book.chapters[0].1.contains("Align performance"));
        assert!(!book.chapters[0].1.contains("DO NOT EXECUTE"));
    }
    #[test]
    fn rejects_traversal_protection_and_corruption_without_panicking() {
        assert!(parse(&fixture(Some(("../outside", "hostile")))).is_err());
        assert!(parse(&fixture(Some(("META-INF/encryption.xml", "protected")))).is_err());
        let bytes = fixture(None);
        for end in (0..bytes.len()).step_by(19) {
            assert!(std::panic::catch_unwind(|| parse(&bytes[..end])).unwrap().is_err());
        }
        assert!(xml("<!DOCTYPE x [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><x>&x;</x>").is_err());
    }
}

/// Snapshot an unpacked EPUB without following links or admitting files outside its root.
/// The resulting archive goes through the same parser and retained-original path as ZIP EPUBs.
pub(super) fn directory_snapshot(root: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::{Seek, Write};
    const MAX_ARCHIVE: usize = 256 * 1024 * 1024;
    struct Bounded(std::io::Cursor<Vec<u8>>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let end = self.0.position().checked_add(bytes.len() as u64).ok_or_else(|| std::io::Error::other("EPUB snapshot size overflow"))?;
            if end > MAX_ARCHIVE as u64 {
                return Err(std::io::Error::other("EPUB snapshot exceeds 256 MiB"));
            }
            self.0.write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Seek for Bounded {
        fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
            self.0.seek(position)
        }
    }
    let metadata = std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("EPUB package must be a regular directory".into());
    }
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut expanded = 0u64;
    let mut entries = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).map_err(|error| error.to_string())? {
            entries = entries.saturating_add(1);
            if entries > MAX_ENTRIES {
                return Err("EPUB package exceeds its entry budget".into());
            }
            let path = entry.map_err(|error| error.to_string())?.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
            if metadata.file_type().is_symlink() {
                return Err("EPUB package cannot contain symbolic links".into());
            }
            let relative = path.strip_prefix(root).map_err(|error| error.to_string())?.to_str().ok_or("EPUB package paths must be UTF-8")?.to_owned();
            safe_path(&relative)?;
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() || metadata.len() > MAX_ENTRY {
                return Err("EPUB package entry is not a bounded regular file".into());
            }
            expanded = expanded.checked_add(metadata.len()).ok_or("EPUB package size overflow")?;
            if expanded > MAX_EXPANDED {
                return Err("EPUB package exceeds its expanded byte budget".into());
            }
            files.push((relative, path));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut archive = zip::ZipWriter::new(Bounded(Cursor::new(Vec::new())));
    for (relative, path) in files {
        let bytes = super::store::read(&path, MAX_ENTRY as usize)?.ok_or("EPUB package changed during import")?;
        archive
            .start_file(relative, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated))
            .map_err(|error| error.to_string())?;
        archive.write_all(&bytes).map_err(|error| error.to_string())?;
    }
    Ok(archive.finish().map_err(|error| error.to_string())?.0.into_inner())
}

#[cfg(test)]
mod package_tests {
    use super::*;
    #[test]
    fn unpacked_book_matches_archive_and_links_are_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("book.epub");
        std::fs::create_dir(&root).unwrap();
        let fixture = super::tests::fixture(None);
        let mut archive = zip::ZipArchive::new(Cursor::new(&fixture)).unwrap();
        for n in 0..archive.len() {
            let mut item = archive.by_index(n).unwrap();
            let path = root.join(item.name());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut bytes = Vec::new();
            item.read_to_end(&mut bytes).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let snapshot = directory_snapshot(&root).unwrap();
        assert_eq!(parse(&snapshot).unwrap().chapters, parse(&fixture).unwrap().chapters);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temporary.path(), root.join("escape")).unwrap();
            assert!(directory_snapshot(&root).unwrap_err().contains("links"));
        }
    }
}
