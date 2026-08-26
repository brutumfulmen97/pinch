use std::io;

use bytes::{Buf, BufMut, Bytes, BytesMut, TryGetError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    key: Bytes,
    val: Bytes,
    deleted: bool,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum EncodeError {
    #[error("key length {length} exceeds the maximum encodable length")]
    KeyTooLarge { length: usize },
    #[error("value length {length} exceeds the maximum encodable length")]
    ValueTooLarge { length: usize },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DecodeError {
    #[error("failed to read WAL entry header")]
    ReadHeader(#[source] io::Error),
    #[error("failed to read WAL entry key")]
    ReadKey(#[source] io::Error),
    #[error("failed to read WAL entry value")]
    ReadValue(#[source] io::Error),
    #[error(
        "WAL entry is too large: key is {key_len} bytes and value is {value_len} bytes; maximum is {maximum} bytes"
    )]
    EntryTooLarge {
        key_len: usize,
        value_len: usize,
        maximum: usize,
    },
}

const MAX_ENTRY_SIZE: usize = 64 * 1024 * 1024;

impl Entry {
    pub(crate) fn new(key: Bytes, val: Bytes, deleted: bool) -> Self {
        Self { key, val, deleted }
    }

    pub(crate) fn key(&self) -> &Bytes {
        &self.key
    }

    pub(crate) fn value(&self) -> &Bytes {
        &self.val
    }

    pub(crate) fn is_deleted(&self) -> bool {
        self.deleted
    }

    pub(crate) fn into_parts(self) -> (Bytes, Bytes, bool) {
        (self.key, self.val, self.deleted)
    }

    pub(crate) fn encode(&self) -> Result<BytesMut, EncodeError> {
        let key_len = self.key.len();
        let val_len = self.val.len();

        if key_len > u32::MAX as usize {
            return Err(EncodeError::KeyTooLarge { length: key_len });
        }

        if val_len > u32::MAX as usize {
            return Err(EncodeError::ValueTooLarge { length: val_len });
        }

        let mut buf = BytesMut::with_capacity(4 + 4 + 1 + key_len + val_len);
        buf.put_u32_le(key_len as u32);
        buf.put_u32_le(val_len as u32);
        if self.deleted {
            buf.put_u8(0x01);
        } else {
            buf.put_u8(0x00);
        }
        buf.put_slice(&self.key);
        buf.put_slice(&self.val);
        Ok(buf)
    }
}

pub(crate) fn decode<R: io::Read>(reader: &mut R) -> Result<Option<Entry>, DecodeError> {
    let mut header = [0; 9];
    match reader
        .read(&mut header[..1])
        .map_err(DecodeError::ReadHeader)?
    {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!("a one-byte buffer cannot receive more than one byte"),
    }
    reader
        .read_exact(&mut header[1..])
        .map_err(DecodeError::ReadHeader)?;

    let key_len =
        u32::from_le_bytes(header[..4].try_into().expect("header has four bytes")) as usize;
    let val_len =
        u32::from_le_bytes(header[4..8].try_into().expect("header has four bytes")) as usize;
    let deleted = match header[8] {
        0x00 => false,
        _ => true,
    };

    if key_len
        .checked_add(val_len)
        .is_none_or(|length| length > MAX_ENTRY_SIZE)
    {
        return Err(DecodeError::EntryTooLarge {
            key_len,
            value_len: val_len,
            maximum: MAX_ENTRY_SIZE,
        });
    }

    let mut key = vec![0; key_len];
    let mut val = vec![0; val_len];
    reader.read_exact(&mut key).map_err(DecodeError::ReadKey)?;
    reader
        .read_exact(&mut val)
        .map_err(DecodeError::ReadValue)?;

    Ok(Some(Entry {
        key: Bytes::from(key),
        val: Bytes::from(val),
        deleted,
    }))
}

pub(crate) fn decode_from_bytes(bytes: &mut Bytes) -> Result<Entry, TryGetError> {
    let key_len = bytes.try_get_u32_le()? as usize;
    let val_len = bytes.try_get_u32_le()? as usize;
    let deleted = match bytes.try_get_u8()? {
        0x00 => false,
        _ => true,
    };

    if bytes.remaining() < key_len + val_len {
        return Err(TryGetError {
            requested: key_len + val_len,
            available: bytes.remaining(),
        });
    }
    let key = bytes.copy_to_bytes(key_len);
    let val = bytes.copy_to_bytes(val_len);
    Ok(Entry { key, val, deleted })
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind};

    use super::*;

    // TODO: Add property tests for encode/decode round trips over arbitrary binary keys and values.
    // TODO: Add a fuzz target to ensure malformed frames never panic the decoder.

    #[test]
    fn entry_encode() -> Result<(), EncodeError> {
        let entry = Entry {
            key: Bytes::from("hello"),
            val: Bytes::from("world"),
            deleted: true,
        };
        let encoded = entry.encode()?;
        assert_eq!(
            encoded.as_ref(),
            b"\x05\x00\x00\x00\x05\x00\x00\x00\x01helloworld"
        );
        Ok(())
    }

    #[test]
    fn entry_decode() -> Result<(), TryGetError> {
        let bytes = b"\x05\x00\x00\x00\x05\x00\x00\x00\x00helloworld";

        let decoded = decode_from_bytes(&mut Bytes::from_static(bytes))?;
        assert_eq!(decoded.key, Bytes::from_static(b"hello"));
        assert_eq!(decoded.val, Bytes::from_static(b"world"));
        assert_eq!(decoded.deleted, false);
        Ok(())
    }

    #[test]
    fn entry_decode_from_reader_returns_entry_then_eof() -> Result<(), DecodeError> {
        let entry = Entry {
            key: Bytes::from_static(b"key"),
            val: Bytes::from_static(b"value"),
            deleted: true,
        };
        let mut reader = Cursor::new(entry.encode().expect("test entry fits in the format"));

        let decoded = decode(&mut reader)?.expect("entry should be present");

        assert_eq!(decoded.key, entry.key);
        assert_eq!(decoded.val, entry.val);
        assert!(decoded.deleted);
        assert!(decode(&mut reader)?.is_none());
        Ok(())
    }

    #[test]
    fn entry_decode_from_reader_rejects_truncated_header() {
        let error = decode(&mut Cursor::new(b"\x05\x00".as_slice())).unwrap_err();

        assert!(matches!(
            error,
            DecodeError::ReadHeader(source) if source.kind() == ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn entry_decode_from_reader_rejects_oversized_entry() {
        let mut bytes = ((MAX_ENTRY_SIZE + 1) as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.push(0);

        let error = decode(&mut Cursor::new(bytes)).unwrap_err();

        assert!(matches!(
            error,
            DecodeError::EntryTooLarge {
                key_len,
                value_len: 0,
                maximum: MAX_ENTRY_SIZE,
            } if key_len == MAX_ENTRY_SIZE + 1
        ));
    }

    #[test]
    fn entry_round_trips_long_binary_data() -> Result<(), TryGetError> {
        let entry = Entry {
            key: Bytes::from((0..=255).collect::<Vec<_>>()),
            val: Bytes::from((0..=255).rev().cycle().take(1_024).collect::<Vec<_>>()),
            deleted: true,
        };

        let encoded = entry.encode().expect("test entry fits in the format");
        let decoded = decode_from_bytes(&mut encoded.freeze())?;

        assert_eq!(decoded.key, entry.key);
        assert_eq!(decoded.val, entry.val);
        assert!(decoded.deleted);
        Ok(())
    }

    #[test]
    fn entry_decode_consumes_only_one_entry() -> Result<(), TryGetError> {
        let first = Entry {
            key: Bytes::from_static(b"first"),
            val: Bytes::from_static(b"value"),
            deleted: false,
        };
        let second = Entry {
            key: Bytes::from_static(b"second"),
            val: Bytes::from_static(b"tombstone"),
            deleted: true,
        };
        let first_encoded = first.encode().expect("test entry fits in the format");
        let second_encoded = second.encode().expect("test entry fits in the format");
        let mut bytes =
            Bytes::from_iter(first_encoded.iter().chain(second_encoded.iter()).copied());

        let decoded_first = decode_from_bytes(&mut bytes)?;

        assert_eq!(decoded_first.key, first.key);
        assert_eq!(decoded_first.val, first.val);
        assert!(!decoded_first.deleted);
        assert_eq!(bytes.as_ref(), second_encoded.as_ref());

        let decoded_second = decode_from_bytes(&mut bytes)?;
        assert_eq!(decoded_second.key, second.key);
        assert_eq!(decoded_second.val, second.val);
        assert!(decoded_second.deleted);
        assert!(bytes.is_empty());
        Ok(())
    }

    #[test]
    fn entry_decode_rejects_truncated_header() {
        let error = decode_from_bytes(&mut Bytes::from_static(b"\x05\x00\x00")).unwrap_err();

        assert_eq!(error.requested, 4);
        assert_eq!(error.available, 3);
    }

    #[test]
    fn entry_decode_rejects_truncated_payload() {
        let bytes = b"\x04\x00\x00\x00\x03\x00\x00\x00\x00abc";
        let error = decode_from_bytes(&mut Bytes::from_static(bytes)).unwrap_err();

        assert_eq!(error.requested, 7);
        assert_eq!(error.available, 3);
    }
}
