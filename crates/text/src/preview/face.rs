//! Bounded temporary face extraction. OpenType 1.9.1 font-file specification:
//! https://learn.microsoft.com/en-us/typography/opentype/spec/otff
//! One temporary SFNT face, no whole-collection read or global font cache.
use std::io::{Read, Seek, SeekFrom};
const MAX_FACE: usize = 64 * 1024 * 1024;
const MAX_TABLES: usize = 256;
fn u16be(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
    let end = offset.checked_add(2).ok_or("offset overflow")?;
    let data = bytes.get(offset..end).ok_or("short integer")?;
    Ok(u16::from_be_bytes([data[0], data[1]]))
}
fn u32be(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    let end = offset.checked_add(4).ok_or("offset overflow")?;
    let data = bytes.get(offset..end).ok_or("short integer")?;
    Ok(u32::from_be_bytes([data[0], data[1], data[2], data[3]]))
}
fn put(out: &mut [u8], offset: usize, data: &[u8]) -> Result<(), &'static str> {
    let end = offset.checked_add(data.len()).ok_or("offset overflow")?;
    out.get_mut(offset..end).ok_or("short output")?.copy_from_slice(data);
    Ok(())
}
fn read_at(r: &mut (impl Read + Seek), offset: u64, out: &mut [u8], file_len: u64) -> Result<(), &'static str> {
    let len = u64::try_from(out.len()).map_err(|_| "length overflow")?;
    if offset.checked_add(len).is_none_or(|end| end > file_len) {
        return Err("table outside font file");
    }
    r.seek(SeekFrom::Start(offset)).map_err(|_| "cannot seek font")?;
    r.read_exact(out).map_err(|_| "cannot read font")
}
fn checksum(bytes: &[u8]) -> u32 {
    let mut sum = 0u32;
    for chunk in bytes.chunks(4) {
        let mut word = [0; 4];
        for (dst, src) in word.iter_mut().zip(chunk) {
            *dst = *src;
        }
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}
struct Table {
    tag: [u8; 4],
    source: u64,
    len: usize,
    dest: usize,
}
pub fn extract(r: &mut (impl Read + Seek), index: u32) -> Result<Vec<u8>, &'static str> {
    let file_len = r.seek(SeekFrom::End(0)).map_err(|_| "cannot size font")?;
    let mut header = [0; 12];
    read_at(r, 0, &mut header, file_len)?;
    let face = if header.starts_with(b"ttcf") {
        let version = u32be(&header, 4)?;
        if !matches!(version, 0x00010000 | 0x00020000) {
            return Err("unsupported collection version");
        }
        let count = u32be(&header, 8)?;
        if count == 0 || count > 256 || index >= count {
            return Err("invalid collection face index/count");
        }
        let header_end = 12u64
            .checked_add(u64::from(count).checked_mul(4).ok_or("offset overflow")?)
            .and_then(|end| end.checked_add(if version == 0x00020000 { 12 } else { 0 }))
            .ok_or("offset overflow")?;
        if header_end > file_len {
            return Err("truncated collection header");
        }
        let offset = 12u64.checked_add(u64::from(index).checked_mul(4).ok_or("offset overflow")?).ok_or("offset overflow")?;
        let mut value = [0; 4];
        read_at(r, offset, &mut value, file_len)?;
        let face = u64::from(u32::from_be_bytes(value));
        if face < header_end {
            return Err("face overlaps collection header");
        }
        face
    } else {
        if index != 0 {
            return Err("single font has no such face");
        }
        0
    };
    read_at(r, face, &mut header, file_len)?;
    if !matches!(header.get(..4), Some([0, 1, 0, 0]) | Some(b"OTTO") | Some(b"true")) {
        return Err("unsupported SFNT version");
    }
    let count = usize::from(u16be(&header, 4)?);
    if count == 0 || count > MAX_TABLES {
        return Err("too many or no font tables");
    }
    let mut directory = vec![0; count.checked_mul(16).ok_or("table count overflow")?];
    read_at(r, face.checked_add(12).ok_or("offset overflow")?, &mut directory, file_len)?;
    let mut tables = Vec::new();
    tables.try_reserve_exact(count).map_err(|_| "not enough memory")?;
    for record in directory.as_chunks::<16>().0 {
        let mut tag = [0; 4];
        tag.copy_from_slice(record.get(..4).ok_or("short record")?);
        if tag == *b"DSIG" {
            continue;
        }
        if tag.iter().any(|byte| !(0x20..=0x7e).contains(byte)) {
            return Err("invalid table tag");
        }
        if tables.iter().any(|table: &Table| table.tag == tag) {
            return Err("duplicate table tag");
        }
        let source = u64::from(u32be(record, 8)?);
        let len = usize::try_from(u32be(record, 12)?).map_err(|_| "table length overflow")?;
        if len > MAX_FACE || source.checked_add(u64::try_from(len).map_err(|_| "length overflow")?).is_none_or(|end| end > file_len) {
            return Err("font table exceeds budget or file");
        }
        tables.push(Table { tag, source, len, dest: 0 });
    }
    tables.sort_by_key(|table| table.tag);
    let count = tables.len();
    if count == 0 {
        return Err("font has no usable tables");
    }
    let mut total = 12usize.checked_add(count.checked_mul(16).ok_or("directory overflow")?).ok_or("directory overflow")?;
    for table in &mut tables {
        table.dest = total;
        let padded = table.len.checked_add(3).ok_or("length overflow")? & !3;
        total = total.checked_add(padded).filter(|size| *size <= MAX_FACE).ok_or("font face exceeds byte budget")?;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(total).map_err(|_| "not enough memory")?;
    out.resize(total, 0);
    put(&mut out, 0, header.get(..4).ok_or("short header")?)?;
    let count16 = u16::try_from(count).map_err(|_| "table count overflow")?;
    put(&mut out, 4, &count16.to_be_bytes())?;
    let power = 1usize.checked_shl(usize::BITS - 1 - count.leading_zeros()).ok_or("search range overflow")?;
    let search = power.checked_mul(16).ok_or("search range overflow")?;
    put(&mut out, 6, &u16::try_from(search).map_err(|_| "search range overflow")?.to_be_bytes())?;
    put(&mut out, 8, &u16::try_from(power.trailing_zeros()).map_err(|_| "selector overflow")?.to_be_bytes())?;
    put(
        &mut out,
        10,
        &u16::try_from(count.checked_mul(16).and_then(|size| size.checked_sub(search)).ok_or("range overflow")?).map_err(|_| "range overflow")?.to_be_bytes(),
    )?;
    let mut head = None;
    for (index, table) in tables.iter().enumerate() {
        let end = table.dest.checked_add(table.len).ok_or("table overflow")?;
        read_at(r, table.source, out.get_mut(table.dest..end).ok_or("short output")?, file_len)?;
        if table.tag == *b"head" {
            if table.len < 12 {
                return Err("short head table");
            }
            put(&mut out, table.dest.checked_add(8).ok_or("offset overflow")?, &[0; 4])?;
            head = Some(table.dest);
        }
        let offset = 12usize.checked_add(index.checked_mul(16).ok_or("directory overflow")?).ok_or("directory overflow")?;
        let sum = checksum(out.get(table.dest..end).ok_or("short output")?);
        put(&mut out, offset, &table.tag)?;
        put(&mut out, offset + 4, &sum.to_be_bytes())?;
        put(&mut out, offset + 8, &u32::try_from(table.dest).map_err(|_| "offset overflow")?.to_be_bytes())?;
        put(&mut out, offset + 12, &u32::try_from(table.len).map_err(|_| "length overflow")?.to_be_bytes())?;
    }
    let head = head.ok_or("missing head table")?;
    let adjustment = 0xB1B0AFBAu32.wrapping_sub(checksum(&out));
    put(&mut out, head.checked_add(8).ok_or("offset overflow")?, &adjustment.to_be_bytes())?;
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    fn font() -> Vec<u8> {
        let mut out = vec![0; 76];
        out[0..4].copy_from_slice(&[0, 1, 0, 0]);
        out[4..6].copy_from_slice(&2u16.to_be_bytes());
        out[12..16].copy_from_slice(b"head");
        out[20..24].copy_from_slice(&44u32.to_be_bytes());
        out[24..28].copy_from_slice(&28u32.to_be_bytes());
        out[28..32].copy_from_slice(b"name");
        out[36..40].copy_from_slice(&72u32.to_be_bytes());
        out[40..44].copy_from_slice(&4u32.to_be_bytes());
        out[72..76].copy_from_slice(b"test");
        out
    }
    #[test]
    fn standalone_roundtrip_and_checksums() {
        let out = extract(&mut Cursor::new(font()), 0).unwrap();
        assert_eq!(checksum(&out), 0xB1B0AFBA);
        let again = extract(&mut Cursor::new(out.clone()), 0).unwrap();
        assert_eq!(out, again);
        assert!(extract(&mut Cursor::new(font()), 1).is_err());
    }
    #[test]
    fn collection_offsets_are_file_relative_and_only_requested_tables_are_read() {
        let single = font();
        let mut collection = vec![0; 1024 * 1024];
        collection[..4].copy_from_slice(b"ttcf");
        collection[4..8].copy_from_slice(&0x00010000u32.to_be_bytes());
        collection[8..12].copy_from_slice(&2u32.to_be_bytes());
        collection[12..16].copy_from_slice(&32u32.to_be_bytes());
        collection[16..20].copy_from_slice(&32u32.to_be_bytes());
        collection[32..108].copy_from_slice(&single);
        collection[52..56].copy_from_slice(&76u32.to_be_bytes());
        collection[68..72].copy_from_slice(&104u32.to_be_bytes());
        struct Counted {
            cursor: Cursor<Vec<u8>>,
            read: usize,
        }
        impl Read for Counted {
            fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
                let n = self.cursor.read(b)?;
                self.read += n;
                Ok(n)
            }
        }
        impl Seek for Counted {
            fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
                self.cursor.seek(p)
            }
        }
        let mut r = Counted { cursor: Cursor::new(collection), read: 0 };
        let out = extract(&mut r, 1).unwrap();
        assert_eq!(checksum(&out), 0xB1B0AFBA);
        assert!(r.read < 256);
        assert_eq!(&out[72..76], b"test");
    }
    #[test]
    fn bundled_real_faces_preserve_all_table_payloads_except_checksum_adjustment() {
        let fonts: [&[u8]; 2] = [crate::fonts::INTER_REGULAR, crate::fonts::JETBRAINS_MONO_REGULAR];
        fn tables(bytes: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
            let count = usize::from(u16be(bytes, 4).unwrap());
            (0..count)
                .filter_map(|i| {
                    let row = &bytes[12 + i * 16..28 + i * 16];
                    let tag: [u8; 4] = row[..4].try_into().unwrap();
                    if tag == *b"DSIG" {
                        return None;
                    }
                    let offset = u32be(row, 8).unwrap() as usize;
                    let len = u32be(row, 12).unwrap() as usize;
                    let mut data = bytes[offset..offset + len].to_vec();
                    if tag == *b"head" {
                        data[8..12].fill(0);
                    }
                    Some((tag, data))
                })
                .collect()
        }
        for font in fonts {
            let out = extract(&mut Cursor::new(font), 0).unwrap();
            assert_eq!(checksum(&out), 0xB1B0AFBA);
            let mut before = tables(font);
            before.sort_by_key(|row| row.0);
            assert_eq!(before, tables(&out));
        }
    }
    #[test]
    fn malformed_fonts_fail_without_panicking_or_input_sized_allocations() {
        let good = font();
        for end in 0..good.len() {
            assert!(std::panic::catch_unwind(|| extract(&mut Cursor::new(&good[..end]), 0)).unwrap().is_err());
        }
        for position in 0..good.len() {
            for bit in 0..8 {
                let mut bad = good.clone();
                bad[position] ^= 1 << bit;
                let _ = std::panic::catch_unwind(|| extract(&mut Cursor::new(bad), 0)).unwrap();
            }
        }
        for value in [u32::MAX, 64 * 1024 * 1024 + 1] {
            let mut bad = good.clone();
            bad[24..28].copy_from_slice(&value.to_be_bytes());
            assert!(extract(&mut Cursor::new(bad), 0).is_err());
        }
        let mut bad = good.clone();
        bad[28..32].copy_from_slice(b"head");
        assert!(extract(&mut Cursor::new(bad), 0).is_err());
    }
}
