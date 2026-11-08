use super::format::ExportSettings;
use super::*;

/// Receives in-memory export output: `(path, bytes)` per finished file.
#[derive(Clone)]
pub struct OutputSink(pub Arc<dyn Fn(&str, Vec<u8>) -> std::io::Result<()> + Send + Sync>);

impl std::fmt::Debug for OutputSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OutputSink")
    }
}

/// How an audio-only export codes its samples.
pub(crate) enum AudioWriter {
    Pcm(pcm::PcmContainer),
    Flac(Box<filmcraft_flac::Encoder>),
}

/// An export's output file: on disk, or in memory for an [`OutputSink`].
pub(crate) enum Out {
    File(std::io::BufWriter<std::fs::File>),
    Mem(std::io::Cursor<Vec<u8>>),
}

impl Out {
    pub(crate) fn create(settings: &ExportSettings) -> Result<Out> {
        if let Some(file) = &settings.output_file {
            let file = file.try_clone().map_err(|e| ExportError::Io(e.to_string()))?;
            return Ok(Out::File(std::io::BufWriter::new(file)));
        }
        Out::create_path(settings, &settings.path)
    }

    /// An output file at `path` (OP-Atom exports write several).
    pub(crate) fn create_path(settings: &ExportSettings, path: &str) -> Result<Out> {
        if settings.sink.is_some() {
            return Ok(Out::Mem(std::io::Cursor::new(Vec::new())));
        }
        if path == settings.path
            && let Some(file) = &settings.output_file
        {
            let file = file.try_clone().map_err(|e| ExportError::Io(e.to_string()))?;
            return Ok(Out::File(std::io::BufWriter::new(file)));
        }
        let f =
            std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path).map_err(|e| ExportError::Io(format!("{path}: {e}")))?;
        Ok(Out::File(std::io::BufWriter::new(f)))
    }

    /// Flush (and hand in-memory output to the sink); returns the file size.
    pub(crate) fn finish(self, settings: &ExportSettings) -> Result<u64> {
        self.finish_path(settings, &settings.path)
    }

    /// Finish AAC edit duration and roll groups in the trailing movie atom. The shared
    /// container writer does not yet expose sample groups. Read only `moov`, not `mdat`.
    pub(crate) fn finalize_aac_timing(&mut self, samples: u64, rate: u32, priming: u32, packet: u32) -> Result<()> {
        use std::io::{Seek, SeekFrom};
        let io = |e: std::io::Error| ExportError::Io(e.to_string());
        match self {
            Out::File(w) => {
                w.flush().map_err(io)?;
                let mut reader = w.get_ref().try_clone().map_err(io)?;
                let (at, moov) = aac_timing::prepare(&mut reader, samples, rate, priming, packet)?;
                w.seek(SeekFrom::Start(at)).map_err(io)?;
                w.write_all(&moov).map_err(io)?;
            }
            Out::Mem(w) => {
                let (at, moov) = aac_timing::prepare(&mut std::io::Cursor::new(w.get_ref().as_slice()), samples, rate, priming, packet)?;
                w.seek(SeekFrom::Start(at)).map_err(io)?;
                w.write_all(&moov).map_err(io)?;
            }
        }
        Ok(())
    }

    pub(crate) fn finish_path(self, settings: &ExportSettings, path: &str) -> Result<u64> {
        match self {
            Out::File(mut w) => {
                w.flush().map_err(|e| ExportError::Io(e.to_string()))?;
                Ok(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
            }
            Out::Mem(c) => write_output(settings, path, c.into_inner()),
        }
    }
}

/// Write one finished output file (to the sink when there is one); returns its size.
pub(crate) fn write_output(settings: &ExportSettings, path: &str, data: Vec<u8>) -> Result<u64> {
    let n = data.len() as u64;
    match &settings.sink {
        Some(sink) => (sink.0)(path, data),
        None => std::fs::write(path, &data),
    }
    .map_err(|e| ExportError::Io(e.to_string()))?;
    Ok(n)
}

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Out::File(w) => w.write(buf),
            Out::Mem(w) => w.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Out::File(w) => w.flush(),
            Out::Mem(w) => w.flush(),
        }
    }
}

impl std::io::Seek for Out {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        match self {
            Out::File(w) => w.seek(pos),
            Out::Mem(w) => w.seek(pos),
        }
    }
}

/// An in-memory writer that stays reachable after the encoder that owns a clone is dropped.
#[derive(Clone, Default)]
pub(crate) struct SharedBuf(pub(crate) Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
