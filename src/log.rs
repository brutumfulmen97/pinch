use std::io::ErrorKind::AlreadyExists;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::{fs, io, path::Path};

use crate::kv_entry;

#[derive(Debug)]
pub(crate) struct Log {
    file: fs::File,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LogError {
    #[error("could not encode log entry")]
    Encode(#[from] kv_entry::EncodeError),
    #[error("log I/O failed")]
    Io(#[from] io::Error),
    #[error("could not decode entry")]
    Decode(#[from] kv_entry::DecodeError),
}

impl Log {
    pub(crate) fn new(file_name: impl AsRef<Path>) -> Result<Self, LogError> {
        let file = match fs::OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .mode(0o644)
            .open(&file_name)
        {
            Ok(file) => {
                file.sync_all()?;
                let parent_directory = file_name
                    .as_ref()
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));

                fs::File::open(parent_directory)?.sync_all()?;
                file
            }
            Err(error) if error.kind() == AlreadyExists => fs::OpenOptions::new()
                .read(true)
                .append(true)
                .open(file_name)?,
            Err(error) => {
                return Err(error.into());
            }
        };
        Ok(Self { file })
    }

    pub(crate) fn write(&mut self, entry: &kv_entry::Entry) -> Result<(), LogError> {
        let encoded = entry.encode()?;
        self.file.write_all(&encoded)?;
        self.file.sync_data()?;
        Ok(())
    }

    pub(crate) fn read(&mut self) -> Result<Option<kv_entry::Entry>, LogError> {
        Ok(kv_entry::decode(&mut self.file)?)
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn log_reads_empty_file_as_eof() -> anyhow::Result<()> {
        let temp_dir = tempdir()?;
        let mut log = Log::new(temp_dir.path().join("wal"))?;

        assert!(log.read()?.is_none());
        Ok(())
    }

    #[test]
    fn log_persists_entries_across_reopen() -> anyhow::Result<()> {
        let temp_dir = tempdir()?;
        let file_name = temp_dir.path().join("wal");
        let mut log = Log::new(&file_name)?;
        log.write(&kv_entry::Entry::new(
            Bytes::from_static(b"first"),
            Bytes::from_static(b"value"),
            false,
        ))?;
        log.write(&kv_entry::Entry::new(
            Bytes::from_static(b"second"),
            Bytes::new(),
            true,
        ))?;

        drop(log);
        let mut log = Log::new(&file_name)?;

        let first = log.read()?.expect("first entry should be present");
        assert_eq!(first.key(), &Bytes::from_static(b"first"));
        assert_eq!(first.value(), &Bytes::from_static(b"value"));
        assert!(!first.is_deleted());

        let second = log.read()?.expect("second entry should be present");
        assert_eq!(second.key(), &Bytes::from_static(b"second"));
        assert!(second.value().is_empty());
        assert!(second.is_deleted());
        assert!(log.read()?.is_none());
        Ok(())
    }
}
