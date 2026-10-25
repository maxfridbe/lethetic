use base64::{Engine, engine::general_purpose::STANDARD};
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;

const MAX_BYTES: u64 = 512 * 1024 * 1024;
const CHUNK_BYTES: usize = 256 * 1024;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub(crate) struct Download {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub file: NamedTempFile,
    size: u64,
    written: u64,
    started: Instant,
}
#[derive(Default)]
pub(crate) struct Downloads(Option<Download>);
impl Downloads {
    pub fn begin(
        &mut self,
        cache: &Path,
        name: String,
        mime: String,
        size: u64,
    ) -> io::Result<String> {
        if self
            .0
            .as_ref()
            .is_some_and(|download| download.started.elapsed() > Duration::from_secs(300))
        {
            self.0 = None;
        }
        if self.0.is_some() {
            return Err(invalid("Another download is being prepared"));
        }
        if size > MAX_BYTES {
            return Err(invalid("Native downloads are limited to 512 MiB"));
        }
        if name.is_empty()
            || name.chars().count() > 180
            || name == "."
            || name == ".."
            || name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        {
            return Err(invalid("Invalid download filename"));
        }
        let mime = mime.split(';').next().unwrap_or("").trim();
        let mime = if mime.len() <= 100
            && mime.contains('/')
            && mime
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-+._".contains(&b))
        {
            mime
        } else {
            "application/octet-stream"
        };
        std::fs::create_dir_all(cache)?;
        let download = Download {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            mime: mime.into(),
            file: tempfile::Builder::new()
                .prefix("lethetic-download-")
                .tempfile_in(cache)?,
            size,
            written: 0,
            started: Instant::now(),
        };
        let id = download.id.clone();
        self.0 = Some(download);
        Ok(id)
    }
    pub fn append(&mut self, id: &str, offset: u64, data: &str) -> io::Result<()> {
        let download = self
            .0
            .as_mut()
            .filter(|d| d.id == id)
            .ok_or_else(|| invalid("Download unavailable"))?;
        if offset != download.written || data.len() > CHUNK_BYTES.div_ceil(3) * 4 {
            return Err(invalid("Invalid download chunk or offset"));
        }
        let bytes = STANDARD
            .decode(data)
            .map_err(|_| invalid("Invalid download encoding"))?;
        if bytes.len() > CHUNK_BYTES || bytes.len() as u64 > download.size - download.written {
            return Err(invalid("Download exceeds its declared size"));
        }
        download.file.write_all(&bytes)?;
        download.written += bytes.len() as u64;
        Ok(())
    }
    pub fn finish(&mut self, id: &str) -> io::Result<Download> {
        if !self.0.as_ref().is_some_and(|d| d.id == id) {
            return Err(invalid("Download unavailable"));
        }
        let mut download = self
            .0
            .take()
            .ok_or_else(|| invalid("Download unavailable"))?;
        if download.written != download.size {
            return Err(invalid("Download is incomplete"));
        }
        download.file.flush()?;
        Ok(download)
    }
    pub fn cancel(&mut self, id: Option<&str>) {
        if id.is_none() || self.0.as_ref().is_some_and(|d| Some(d.id.as_str()) == id) {
            self.0 = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_chunks_and_empty_files_survive_without_text_conversion() {
        let cache = tempfile::tempdir().unwrap();
        let mut downloads = Downloads::default();
        let bytes: Vec<u8> = (0..700_000).map(|i| (i % 256) as u8).collect();
        let id = downloads
            .begin(
                cache.path(),
                "image.bin".into(),
                "application/octet-stream".into(),
                bytes.len() as u64,
            )
            .unwrap();
        for (i, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
            downloads
                .append(&id, (i * CHUNK_BYTES) as u64, &STANDARD.encode(chunk))
                .unwrap();
        }
        let file = downloads.finish(&id).unwrap();
        let path = file.file.path().to_path_buf();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(file);
        assert!(!path.exists());
        let id = downloads
            .begin(
                cache.path(),
                "empty.txt".into(),
                "text/plain;charset=utf-8".into(),
                0,
            )
            .unwrap();
        let file = downloads.finish(&id).unwrap();
        assert_eq!(file.mime, "text/plain");
        assert_eq!(file.file.as_file().metadata().unwrap().len(), 0);
    }
    #[test]
    fn invalid_chunks_cancellation_and_incomplete_files_do_not_escape_staging() {
        let cache = tempfile::tempdir().unwrap();
        let mut downloads = Downloads::default();
        let id = downloads
            .begin(
                cache.path(),
                "output.zip".into(),
                "application/zip".into(),
                2,
            )
            .unwrap();
        assert!(downloads.append("wrong", 0, "AA==").is_err());
        assert!(downloads.append(&id, 1, "AA==").is_err());
        assert!(downloads.append(&id, 0, "bad!").is_err());
        assert!(downloads.append(&id, 0, "AAAA").is_err());
        assert!(
            downloads
                .append(&id, 0, &"A".repeat(CHUNK_BYTES.div_ceil(3) * 4 + 4))
                .is_err()
        );
        downloads.append(&id, 0, "AA==").unwrap();
        assert!(downloads.finish(&id).is_err());
        assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
        let id = downloads
            .begin(cache.path(), "cancel.txt".into(), "text/plain".into(), 0)
            .unwrap();
        downloads.cancel(Some("wrong"));
        assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 1);
        downloads.cancel(Some(&id));
        assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
    }
    #[test]
    fn filenames_sizes_and_concurrent_downloads_are_bounded() {
        let cache = tempfile::tempdir().unwrap();
        let mut downloads = Downloads::default();
        for name in ["", "..", "../secret", "a\\b", "a\nb"] {
            assert!(
                downloads
                    .begin(cache.path(), name.into(), "text/plain".into(), 0)
                    .is_err()
            );
        }
        assert!(
            downloads
                .begin(
                    cache.path(),
                    "large".into(),
                    "text/plain".into(),
                    MAX_BYTES + 1
                )
                .is_err()
        );
        downloads
            .begin(cache.path(), "ok".into(), "bad\r\nheader".into(), 0)
            .unwrap();
        assert!(
            downloads
                .begin(cache.path(), "another".into(), "text/plain".into(), 0)
                .is_err()
        );
        downloads.cancel(None);
        assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
    }
}
