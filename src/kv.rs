use bytes::Bytes;
use std::{collections::HashMap, path::Path};

use crate::{kv_entry, log};

#[derive(Debug)]
pub(crate) struct Kv {
    log: log::Log,
    entries: HashMap<Bytes, Bytes>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum KvError {
    #[error("key-value store log operation failed")]
    Log(#[from] log::LogError),
}

impl Kv {
    pub(crate) fn new(file_name: impl AsRef<Path>) -> Result<Self, KvError> {
        let mut key_value_store = Self {
            log: log::Log::new(file_name)?,
            entries: HashMap::new(),
        };
        key_value_store.replay()?;
        Ok(key_value_store)
    }

    fn replay(&mut self) -> Result<(), KvError> {
        while let Some(entry) = self.log.read()? {
            let (key, value, deleted) = entry.into_parts();
            if deleted {
                self.entries.remove(&key);
            } else {
                self.entries.insert(key, value);
            }
        }

        Ok(())
    }

    pub(crate) fn get(&self, key: &Bytes) -> Option<Bytes> {
        self.entries.get(key).cloned()
    }

    pub(crate) fn set(&mut self, key: Bytes, value: Bytes) -> Result<bool, KvError> {
        self.log
            .write(&kv_entry::Entry::new(key.clone(), value.clone(), false))?;

        Ok(self.entries.insert(key, value).is_some())
    }

    pub(crate) fn delete(&mut self, key: &Bytes) -> Result<bool, KvError> {
        if self.entries.contains_key(key) {
            self.log
                .write(&kv_entry::Entry::new(key.clone(), Bytes::new(), true))?;
            self.entries.remove(key);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn kv_sets_updates_and_deletes_values() -> anyhow::Result<()> {
        let temp_dir = tempdir()?;
        let mut key_value_store = Kv::new(temp_dir.path().join("wal"))?;
        let key = Bytes::from_static(b"key");

        assert_eq!(key_value_store.get(&key), None);
        assert!(!key_value_store.set(key.clone(), Bytes::from_static(b"first"))?);
        assert_eq!(
            key_value_store.get(&key),
            Some(Bytes::from_static(b"first"))
        );
        assert!(key_value_store.set(key.clone(), Bytes::from_static(b"second"))?);
        assert_eq!(
            key_value_store.get(&key),
            Some(Bytes::from_static(b"second"))
        );
        assert!(key_value_store.delete(&key)?);
        assert_eq!(key_value_store.get(&key), None);
        assert!(!key_value_store.delete(&key)?);
        Ok(())
    }

    #[test]
    fn kv_replays_persisted_entries_after_reopen() -> anyhow::Result<()> {
        let temp_dir = tempdir()?;
        let file_name = temp_dir.path().join("wal");
        let kept = Bytes::from_static(b"kept");
        let deleted = Bytes::from_static(b"deleted");

        let mut first = Kv::new(&file_name)?;
        first.set(kept.clone(), Bytes::from_static(b"value"))?;
        first.set(deleted.clone(), Bytes::from_static(b"old-value"))?;
        first.delete(&deleted)?;
        drop(first);

        let reopened = Kv::new(&file_name)?;

        assert_eq!(reopened.get(&kept), Some(Bytes::from_static(b"value")));
        assert_eq!(reopened.get(&deleted), None);
        Ok(())
    }
}
