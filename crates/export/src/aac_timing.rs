//! Complete the AAC timing metadata after the shared MP4 writer finishes `moov`.
//!
//! Apple QuickTime File Format, “Using track structures to represent encoder delay
//! explicitly”: an edit list alone is ambiguous. A `roll` sample description and mapping
//! signal that the edit starts after the encoder's actual delay; its duration excludes
//! the padded final packet. The shared writer currently exposes neither sample group.
//! https://developer.apple.com/documentation/quicktime-file-format/using_track_structures_to_represent_encode_delay_explictly
//!
//! QuickTime AAC also needs a version 1 sound description with compressed-frame tables and
//! a `wave` decompression parameter; the shared writer currently emits the ISO form.
//! Its `moov` is last, so this reads only that bounded atom and appends metadata there.
//! Media data, chunk offsets, and video sample tables stay where the writer put them.

use std::io::{Read, Seek, SeekFrom};

use crate::{ExportError, Result};

const MAX_MOOV: u64 = 32 * 1024 * 1024;
const MAX_BOXES: usize = 1_000_000;

fn bad(reason: &str) -> ExportError {
    ExportError::Encode(format!("AAC timing metadata: {reason}"))
}

fn u32_at(data: &[u8], at: usize) -> Result<u32> {
    let b = data.get(at..at.saturating_add(4)).ok_or_else(|| bad("truncated integer"))?;
    Ok(u32::from_be_bytes(b.try_into().map_err(|_| bad("truncated integer"))?))
}

fn fourcc(data: &[u8], at: usize) -> Result<[u8; 4]> {
    let b = data.get(at..at.saturating_add(4)).ok_or_else(|| bad("truncated atom type"))?;
    b.try_into().map_err(|_| bad("truncated atom type"))
}

fn children(data: &[u8]) -> Result<Vec<([u8; 4], &[u8])>> {
    let mut result = Vec::new();
    let mut at = 0usize;
    while at < data.len() {
        if result.len() >= MAX_BOXES {
            return Err(bad("too many atoms"));
        }
        let size = usize::try_from(u32_at(data, at)?).map_err(|_| bad("atom size exceeds address space"))?;
        let kind = fourcc(data, at.checked_add(4).ok_or_else(|| bad("atom offset overflow"))?)?;
        let end = at.checked_add(size).ok_or_else(|| bad("atom size overflow"))?;
        if size < 8 || end > data.len() {
            return Err(bad("invalid atom size"));
        }
        result.push((kind, data.get(at..end).ok_or_else(|| bad("truncated atom"))?));
        at = end;
    }
    Ok(result)
}

fn atom(kind: [u8; 4], body: &[u8]) -> Result<Vec<u8>> {
    let n = body.len().checked_add(8).ok_or_else(|| bad("atom length overflow"))?;
    let size = u32::try_from(n).map_err(|_| bad("atom exceeds 32-bit size"))?;
    let mut out = Vec::new();
    out.try_reserve_exact(n).map_err(|_| bad("cannot allocate atom"))?;
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(body);
    Ok(out)
}

fn put_u32(raw: &[u8], offset: usize, n: u32) -> Result<Vec<u8>> {
    let end = offset.checked_add(4).ok_or_else(|| bad("field offset overflow"))?;
    if raw.get(offset..end).is_none() {
        return Err(bad("truncated duration field"));
    }
    let mut out = raw.to_vec();
    if let Some(field) = out.get_mut(offset..end) {
        field.copy_from_slice(&n.to_be_bytes());
    }
    Ok(out)
}

fn full_v0(raw: &[u8]) -> Result<()> {
    if raw.get(8) != Some(&0) {
        return Err(bad("unsupported atom version"));
    }
    Ok(())
}

fn only<'a>(parts: &'a [([u8; 4], &'a [u8])], kind: [u8; 4]) -> Result<&'a [u8]> {
    let mut matches = parts.iter().filter(|(t, _)| *t == kind);
    let first = matches.next().ok_or_else(|| bad("missing required atom"))?.1;
    if matches.next().is_some() {
        return Err(bad("duplicate required atom"));
    }
    Ok(first)
}

fn is_audio(trak: &[u8]) -> Result<bool> {
    let tracks = children(trak.get(8..).ok_or_else(|| bad("truncated track"))?)?;
    let mdia = only(&tracks, *b"mdia")?;
    let mdia_parts = children(mdia.get(8..).ok_or_else(|| bad("truncated media"))?)?;
    let hdlr = only(&mdia_parts, *b"hdlr")?;
    Ok(fourcc(hdlr, 16)? == *b"soun")
}

fn sample_groups(count: u32) -> Result<Vec<u8>> {
    if count == 0 {
        return Err(bad("AAC has no packets"));
    }
    // sgpd version 1, default_length = 2, one AudioRollRecovery entry at -1.
    let mut description = Vec::from(&b"\x01\0\0\0roll"[..]);
    description.extend_from_slice(&2u32.to_be_bytes());
    description.extend_from_slice(&1u32.to_be_bytes());
    description.extend_from_slice(&(-1i16).to_be_bytes());
    let sgpd = atom(*b"sgpd", &description)?;
    // sbgp version 0: one run covering every AAC packet, description index 1.
    let mut mapping = Vec::from(&b"\0\0\0\0roll"[..]);
    mapping.extend_from_slice(&1u32.to_be_bytes());
    mapping.extend_from_slice(&count.to_be_bytes());
    mapping.extend_from_slice(&1u32.to_be_bytes());
    let mut both = sgpd;
    both.extend_from_slice(&atom(*b"sbgp", &mapping)?);
    Ok(both)
}

/// Apple's QuickTime File Format, “Sound sample data” and “Sound sample description version 1”:
/// AAC uses compression ID -2 and a `wave` atom containing its `esds` and a terminator.
/// Only the known ISO AAC entry emitted by the shared writer is accepted here.
fn quicktime_aac_description(stsd: &[u8]) -> Result<Vec<u8>> {
    full_v0(stsd)?;
    if u32_at(stsd, 12)? != 1 {
        return Err(bad("QuickTime AAC requires one sample description"));
    }
    let entry = stsd.get(16..).ok_or_else(|| bad("truncated AAC description"))?;
    if entry.len() < 36 || usize::try_from(u32_at(entry, 0)?).ok() != Some(entry.len()) || fourcc(entry, 4)? != *b"mp4a" {
        return Err(bad("unsupported AAC sample description"));
    }
    if entry.get(16..18) != Some(&[0, 0][..]) || entry.get(28..30) != Some(&[0, 0][..]) {
        return Err(bad("unsupported AAC sound description version or compression ID"));
    }
    let extras = children(entry.get(36..).ok_or_else(|| bad("truncated AAC extensions"))?)?;
    if extras.len() != 1 || extras[0].0 != *b"esds" {
        return Err(bad("expected one AAC elementary stream descriptor"));
    }
    let mut base = entry.get(8..36).ok_or_else(|| bad("truncated AAC sound fields"))?.to_vec();
    base.get_mut(8..10).ok_or_else(|| bad("truncated sound version"))?.copy_from_slice(&1u16.to_be_bytes());
    base.get_mut(20..22).ok_or_else(|| bad("truncated compression ID"))?.copy_from_slice(&(-2i16).to_be_bytes());
    base.extend_from_slice(&1024u32.to_be_bytes()); // AAC-LC samples per compressed packet.
    base.extend_from_slice(&[0; 12]); // Variable-size packet/frame/sample byte counts.
    let mut wave = atom(*b"frma", b"mp4a")?;
    wave.extend_from_slice(extras[0].1);
    wave.extend_from_slice(&atom([0; 4], &[])?);
    base.extend_from_slice(&atom(*b"wave", &wave)?);
    let entry = atom(*b"mp4a", &base)?;
    let mut body = stsd.get(8..16).ok_or_else(|| bad("truncated sample description header"))?.to_vec();
    body.extend_from_slice(&entry);
    atom(*b"stsd", &body)
}

#[derive(Default)]
struct Seen {
    audio_tracks: usize,
    edits: usize,
    sample_tables: usize,
    packets: u32,
    media_duration: u32,
}

fn rebuild(raw: &[u8], kind: [u8; 4], audio: bool, quicktime: bool, duration: u32, priming: u32, rate: u32, seen: &mut Seen) -> Result<Vec<u8>> {
    if audio && quicktime && kind == *b"stsd" {
        return quicktime_aac_description(raw);
    }
    if audio && kind == *b"tkhd" {
        full_v0(raw)?;
        return put_u32(raw, 28, duration);
    }
    if audio && kind == *b"elst" {
        full_v0(raw)?;
        if u32_at(raw, 12)? != 1 || u32_at(raw, 20)? != priming {
            return Err(bad("unexpected AAC edit list"));
        }
        seen.edits = seen.edits.checked_add(1).ok_or_else(|| bad("too many edit lists"))?;
        return put_u32(raw, 16, duration);
    }
    if audio && kind == *b"mdhd" {
        full_v0(raw)?;
        if u32_at(raw, 20)? != rate {
            return Err(bad("AAC media timescale mismatch"));
        }
        seen.media_duration = u32_at(raw, 24)?;
        return Ok(raw.to_vec());
    }
    if !matches!(&kind, b"trak" | b"edts" | b"mdia" | b"minf" | b"stbl") {
        return Ok(raw.to_vec());
    }
    let contents = raw.get(8..).ok_or_else(|| bad("truncated container"))?;
    let parts = children(contents)?;
    let audio = if kind == *b"trak" { is_audio(raw)? } else { audio };
    if kind == *b"trak" && audio {
        seen.audio_tracks = seen.audio_tracks.checked_add(1).ok_or_else(|| bad("too many audio tracks"))?;
    }
    let mut payload = Vec::new();
    for (child_kind, child) in &parts {
        payload.extend_from_slice(&rebuild(child, *child_kind, audio, quicktime, duration, priming, rate, seen)?);
    }
    if audio && kind == *b"stbl" {
        if parts.iter().any(|(t, _)| *t == *b"sgpd" || *t == *b"sbgp") {
            return Err(bad("AAC sample groups already present"));
        }
        let stsd = only(&parts, *b"stsd")?;
        if fourcc(stsd, 20)? != *b"mp4a" {
            return Err(bad("audio track is not AAC"));
        }
        let stsz = only(&parts, *b"stsz")?;
        full_v0(stsz)?;
        seen.sample_tables = seen.sample_tables.checked_add(1).ok_or_else(|| bad("too many AAC sample tables"))?;
        seen.packets = u32_at(stsz, 16)?;
        payload.extend_from_slice(&sample_groups(seen.packets)?);
    }
    atom(kind, &payload)
}

fn patch_moov(moov: &[u8], samples: u64, rate: u32, priming: u32, quicktime: bool) -> Result<Vec<u8>> {
    if fourcc(moov, 4)? != *b"moov" || usize::try_from(u32_at(moov, 0)?).ok() != Some(moov.len()) {
        return Err(bad("invalid movie atom"));
    }
    let parts = children(moov.get(8..).ok_or_else(|| bad("truncated movie"))?)?;
    let mvhd = only(&parts, *b"mvhd")?;
    full_v0(mvhd)?;
    let movie_rate = u32_at(mvhd, 20)?;
    if rate == 0 || movie_rate == 0 {
        return Err(bad("zero timescale"));
    }
    let ticks =
        u128::from(samples).checked_mul(u128::from(movie_rate)).and_then(|v| v.checked_add(u128::from(rate / 2))).ok_or_else(|| bad("duration overflow"))?
            / u128::from(rate);
    let duration = u32::try_from(ticks).map_err(|_| bad("AAC edit exceeds movie timescale range"))?;
    let mut seen = Seen::default();
    let mut changed = Vec::new();
    let mut longest = 0u32;
    for (kind, raw) in &parts {
        let item = rebuild(raw, *kind, false, quicktime, duration, priming, rate, &mut seen)?;
        if *kind == *b"trak" {
            let traks = children(item.get(8..).ok_or_else(|| bad("truncated rebuilt track"))?)?;
            let tkhd = only(&traks, *b"tkhd")?;
            full_v0(tkhd)?;
            longest = longest.max(u32_at(tkhd, 28)?);
        }
        changed.push((*kind, item));
    }
    if seen.audio_tracks != 1 || seen.edits != 1 || seen.sample_tables != 1 {
        return Err(bad("expected one complete AAC audio track"));
    }
    let expected_packets = samples.div_ceil(1024).checked_add(1).ok_or_else(|| bad("AAC packet count overflow"))?;
    if u64::from(seen.packets) != expected_packets || u64::from(seen.media_duration) != expected_packets.saturating_mul(1024) {
        return Err(bad("AAC packet table does not match the source waveform"));
    }
    let mut payload = Vec::new();
    for (kind, item) in changed {
        payload.extend_from_slice(&if kind == *b"mvhd" { put_u32(&item, 24, longest)? } else { item });
    }
    atom(*b"moov", &payload)
}

/// Locate the trailing movie atom and prepare corrected bytes. The caller writes them at the
/// returned offset after the MP4 writer has finished. No media payload is read into memory.
pub(crate) fn prepare<R: Read + Seek>(input: &mut R, samples: u64, rate: u32, priming: u32, packet: u32) -> Result<(u64, Vec<u8>)> {
    if priming != 1024 || packet != 1024 {
        return Err(bad("unsupported AAC encoder delay or packet size"));
    }
    let len = input.seek(SeekFrom::End(0)).map_err(|e| ExportError::Io(e.to_string()))?;
    let mut pos = 0u64;
    let mut moov_at = None;
    let mut saw_mdat = false;
    let mut quicktime = false;
    let mut saw_ftyp = false;
    for _ in 0..128 {
        if pos == len {
            break;
        }
        if len.saturating_sub(pos) < 8 {
            return Err(bad("truncated top-level atom"));
        }
        input.seek(SeekFrom::Start(pos)).map_err(|e| ExportError::Io(e.to_string()))?;
        let mut header = [0u8; 16];
        input.read_exact(&mut header[..8]).map_err(|e| ExportError::Io(e.to_string()))?;
        let size32 = u32::from_be_bytes(header[..4].try_into().map_err(|_| bad("invalid atom header"))?);
        let kind = fourcc(&header, 4)?;
        let (size, min_size) = if size32 == 1 {
            if len.saturating_sub(pos) < 16 {
                return Err(bad("truncated extended atom"));
            }
            input.read_exact(&mut header[8..]).map_err(|e| ExportError::Io(e.to_string()))?;
            (u64::from_be_bytes(header[8..].try_into().map_err(|_| bad("invalid extended atom"))?), 16)
        } else {
            (u64::from(size32), 8)
        };
        let end = pos.checked_add(size).ok_or_else(|| bad("top-level size overflow"))?;
        if size < min_size || end > len {
            return Err(bad("invalid top-level atom size"));
        }
        if kind == *b"mdat" {
            saw_mdat = true;
        }
        if kind == *b"ftyp" {
            if saw_ftyp || size < 12 {
                return Err(bad("duplicate or truncated file type atom"));
            }
            saw_ftyp = true;
            input.seek(SeekFrom::Start(pos.checked_add(8).ok_or_else(|| bad("file type offset overflow"))?)).map_err(|e| ExportError::Io(e.to_string()))?;
            let mut major = [0u8; 4];
            input.read_exact(&mut major).map_err(|e| ExportError::Io(e.to_string()))?;
            quicktime = major == *b"qt  ";
        }
        if kind == *b"moov" {
            if moov_at.is_some() || end != len || size > MAX_MOOV || size32 == 1 {
                return Err(bad("unsupported movie atom layout or size"));
            }
            moov_at = Some((pos, size));
        }
        pos = end;
    }
    if pos != len || !saw_mdat {
        return Err(bad("incomplete MP4 atom table"));
    }
    let (at, size) = moov_at.ok_or_else(|| bad("missing trailing movie atom"))?;
    let size = usize::try_from(size).map_err(|_| bad("movie atom exceeds address space"))?;
    let mut moov = Vec::new();
    moov.try_reserve_exact(size).map_err(|_| bad("cannot allocate movie atom"))?;
    moov.resize(size, 0);
    input.seek(SeekFrom::Start(at)).map_err(|e| ExportError::Io(e.to_string()))?;
    input.read_exact(&mut moov).map_err(|e| ExportError::Io(e.to_string()))?;
    Ok((at, patch_moov(&moov, samples, rate, priming, quicktime)?))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn make(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        atom(*kind, body).unwrap()
    }

    fn fake_export() -> Vec<u8> {
        let mut mvhd = vec![0; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&3008u32.to_be_bytes());
        let mut video_tkhd = vec![0; 84];
        video_tkhd[20..24].copy_from_slice(&3000u32.to_be_bytes());
        let mut video_hdlr = vec![0; 16];
        video_hdlr[8..12].copy_from_slice(b"vide");
        let video = make(b"trak", &[make(b"tkhd", &video_tkhd), make(b"mdia", &make(b"hdlr", &video_hdlr))].concat());

        let mut audio_tkhd = vec![0; 84];
        audio_tkhd[20..24].copy_from_slice(&3008u32.to_be_bytes());
        let mut elst = vec![0; 20];
        elst[4..8].copy_from_slice(&1u32.to_be_bytes());
        elst[8..12].copy_from_slice(&3008u32.to_be_bytes());
        elst[12..16].copy_from_slice(&1024u32.to_be_bytes());
        let mut mdhd = vec![0; 24];
        mdhd[12..16].copy_from_slice(&48000u32.to_be_bytes());
        mdhd[16..20].copy_from_slice(&145408u32.to_be_bytes());
        let mut audio_hdlr = vec![0; 16];
        audio_hdlr[8..12].copy_from_slice(b"soun");
        let mut sound = vec![0; 36];
        sound[4..8].copy_from_slice(b"mp4a");
        sound[14..16].copy_from_slice(&1u16.to_be_bytes());
        sound[24..26].copy_from_slice(&2u16.to_be_bytes());
        sound[26..28].copy_from_slice(&16u16.to_be_bytes());
        sound[32..36].copy_from_slice(&(48_000u32 << 16).to_be_bytes());
        sound.extend_from_slice(&make(b"esds", &[0; 8]));
        let size = u32::try_from(sound.len()).unwrap();
        sound[0..4].copy_from_slice(&size.to_be_bytes());
        let mut stsd = vec![0; 8];
        stsd[4..8].copy_from_slice(&1u32.to_be_bytes());
        stsd.extend_from_slice(&sound);
        let mut stsz = vec![0; 12];
        stsz[8..12].copy_from_slice(&142u32.to_be_bytes());
        let stbl = make(b"stbl", &[make(b"stsd", &stsd), make(b"stsz", &stsz)].concat());
        let audio_mdia = make(b"mdia", &[make(b"mdhd", &mdhd), make(b"hdlr", &audio_hdlr), make(b"minf", &stbl)].concat());
        let audio = make(b"trak", &[make(b"tkhd", &audio_tkhd), make(b"edts", &make(b"elst", &elst)), audio_mdia].concat());
        let moov = make(b"moov", &[make(b"mvhd", &mvhd), video, audio].concat());
        [make(b"ftyp", b"isom"), make(b"mdat", b"untouched media"), moov].concat()
    }

    #[test]
    fn explicit_delay_and_exact_source_duration() {
        let original = fake_export();
        let (at, fixed) = prepare(&mut Cursor::new(&original), 144000, 48000, 1024, 1024).unwrap();
        assert_eq!(&original[..at as usize], &fake_export()[..at as usize]);
        let moov = children(&fixed[8..]).unwrap();
        assert_eq!(u32_at(only(&moov, *b"mvhd").unwrap(), 24).unwrap(), 3000);
        let tracks: Vec<_> = moov.iter().filter(|(kind, _)| *kind == *b"trak").collect();
        assert_eq!(tracks.len(), 2);
        let audio = children(&tracks[1].1[8..]).unwrap();
        assert_eq!(u32_at(only(&audio, *b"tkhd").unwrap(), 28).unwrap(), 3000);
        let edts = children(&only(&audio, *b"edts").unwrap()[8..]).unwrap();
        assert_eq!(u32_at(only(&edts, *b"elst").unwrap(), 16).unwrap(), 3000);
        let mdia = children(&only(&audio, *b"mdia").unwrap()[8..]).unwrap();
        let minf = children(&only(&mdia, *b"minf").unwrap()[8..]).unwrap();
        let stbl = children(&only(&minf, *b"stbl").unwrap()[8..]).unwrap();
        let sgpd = only(&stbl, *b"sgpd").unwrap();
        let sbgp = only(&stbl, *b"sbgp").unwrap();
        assert_eq!(&sgpd[8..], b"\x01\0\0\0roll\0\0\0\x02\0\0\0\x01\xff\xff");
        assert_eq!(&sbgp[8..], b"\0\0\0\0roll\0\0\0\x01\0\0\0\x8e\0\0\0\x01");
    }

    #[test]
    fn hostile_or_incomplete_atoms_return_errors_without_panicking() {
        let good = fake_export();
        for len in 0..good.len() {
            let bytes = &good[..len];
            let checked = std::panic::catch_unwind(|| prepare(&mut Cursor::new(bytes), 144000, 48000, 1024, 1024));
            assert!(checked.is_ok(), "panicked after {len} bytes");
            assert!(checked.unwrap().is_err(), "accepted {len} bytes");
        }
        for (rate, priming, packet) in [(0, 1024, 1024), (48000, 2112, 1024), (48000, 1024, 0)] {
            assert!(prepare(&mut Cursor::new(&good), 144000, rate, priming, packet).is_err());
        }
        let mut corrupt = good;
        corrupt[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(prepare(&mut Cursor::new(&corrupt), 144000, 48000, 1024, 1024).is_err());
    }

    #[test]
    fn quicktime_aac_uses_version_one_wave_and_preserves_media() {
        let mut original = fake_export();
        original[8..12].copy_from_slice(b"qt  ");
        let (at, fixed) = prepare(&mut Cursor::new(&original), 144_000, 48_000, 1024, 1024).unwrap();
        assert_eq!(&original[..at as usize], &fake_export_with_qt_prefix()[..at as usize]);
        let moov = children(&fixed[8..]).unwrap();
        let tracks: Vec<_> = moov.iter().filter(|(kind, _)| *kind == *b"trak").collect();
        let mdia = children(&tracks[1].1[8..]).unwrap();
        let minf = children(&only(&mdia, *b"mdia").unwrap()[8..]).unwrap();
        let stbl = children(&only(&minf, *b"minf").unwrap()[8..]).unwrap();
        let table = children(&only(&stbl, *b"stbl").unwrap()[8..]).unwrap();
        let stsd = only(&table, *b"stsd").unwrap();
        let entry = &stsd[16..];
        assert_eq!(u32_at(entry, 0).unwrap() as usize, entry.len());
        assert_eq!(&entry[16..18], &1u16.to_be_bytes());
        assert_eq!(&entry[28..30], &(-2i16).to_be_bytes());
        assert_eq!(u32_at(entry, 36).unwrap(), 1024);
        let ext = children(&entry[52..]).unwrap();
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0].0, *b"wave");
        let wave = children(&ext[0].1[8..]).unwrap();
        assert_eq!(wave.iter().map(|(tag, _)| *tag).collect::<Vec<_>>(), [*b"frma", *b"esds", [0; 4]]);
        assert_eq!(&wave[0].1[8..], b"mp4a");
    }

    fn fake_export_with_qt_prefix() -> Vec<u8> {
        let mut data = fake_export();
        data[8..12].copy_from_slice(b"qt  ");
        data
    }

    #[test]
    fn quicktime_description_rejects_corruption_without_panicking() {
        let good = fake_export_with_qt_prefix();
        let (at, _) = prepare(&mut Cursor::new(&good), 144_000, 48_000, 1024, 1024).unwrap();
        let moov = children(&good[at as usize + 8..]).unwrap();
        let tracks: Vec<_> = moov.iter().filter(|(kind, _)| *kind == *b"trak").collect();
        let mdia = children(&tracks[1].1[8..]).unwrap();
        let minf = children(&only(&mdia, *b"mdia").unwrap()[8..]).unwrap();
        let stbl = children(&only(&minf, *b"minf").unwrap()[8..]).unwrap();
        let table = children(&only(&stbl, *b"stbl").unwrap()[8..]).unwrap();
        let stsd = only(&table, *b"stsd").unwrap();
        for len in 0..stsd.len() {
            let result = std::panic::catch_unwind(|| quicktime_aac_description(&stsd[..len]));
            assert!(result.is_ok(), "panicked at length {len}");
            assert!(result.unwrap().is_err(), "accepted truncated description at {len}");
        }
        for index in [0, 3, 12, 15, 16, 19, 20, 23, 32, 34, 52] {
            let mut damaged = stsd.to_vec();
            damaged[index] ^= 0xff;
            assert!(std::panic::catch_unwind(|| quicktime_aac_description(&damaged)).is_ok());
        }
    }
}
