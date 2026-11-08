//! Input limits are enforced while reading, including files that grow after metadata inspection.
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

pub(crate) fn size(len: u64, limit: usize) -> io::Result<usize> {
    usize::try_from(len)
        .ok()
        .filter(|len| *len <= limit)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("file exceeds the {limit} byte limit")))
}

pub(crate) fn read(reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let cap = limit.checked_add(1).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid file limit"))?;
    let mut bytes = Vec::new();
    reader.take(cap as u64).read_to_end(&mut bytes)?;
    size(bytes.len() as u64, limit)?;
    Ok(bytes)
}

pub(crate) fn file(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    size(file.metadata()?.len(), limit)?;
    read(file, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growing_input_stops_at_the_limit_without_reading_the_rest() {
        let mut endless = io::repeat(0);
        assert_eq!(read(&mut endless, 32).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(read(&b"valid"[..], 5).unwrap(), b"valid");
        assert_eq!(read(&b""[..], 0).unwrap(), b"");
        assert!(read(&b"x"[..], 0).is_err());
        assert!(size(u64::MAX, 32).is_err());
    }
}
