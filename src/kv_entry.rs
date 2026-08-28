use std::io;

use bytes::{BufMut, Bytes, BytesMut};

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
    #[error(
        "WAL entry is too large: key is {key_len} bytes and value is {value_len} bytes; maximum is {maximum} bytes"
    )]
    EntryTooLarge {
        key_len: usize,
        value_len: usize,
        maximum: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DecodeError {
    #[error("WAL entry checksum mismatch")]
    ChecksumMismatch,
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
const CHECKSUM_SIZE: usize = 4;
const HEADER_SIZE: usize = CHECKSUM_SIZE + 4 + 4 + 1;

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

        if key_len
            .checked_add(val_len)
            .is_none_or(|length| length > MAX_ENTRY_SIZE)
        {
            return Err(EncodeError::EntryTooLarge {
                key_len,
                value_len: val_len,
                maximum: MAX_ENTRY_SIZE,
            });
        }

        let mut buf = BytesMut::with_capacity(HEADER_SIZE + key_len + val_len);
        buf.put_u32_le(0);
        buf.put_u32_le(key_len as u32);
        buf.put_u32_le(val_len as u32);
        if self.deleted {
            buf.put_u8(0x01);
        } else {
            buf.put_u8(0x00);
        }
        buf.put_slice(&self.key);
        buf.put_slice(&self.val);
        let checksum = crc32fast::hash(&buf[CHECKSUM_SIZE..]);
        buf[..CHECKSUM_SIZE].copy_from_slice(&checksum.to_le_bytes());
        Ok(buf)
    }
}

pub(crate) fn decode<R: io::Read>(reader: &mut R) -> Result<Option<Entry>, DecodeError> {
    let mut header = [0; HEADER_SIZE];
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

    let checksum = u32::from_le_bytes(header[..4].try_into().expect("header has four bytes"));
    let key_len =
        u32::from_le_bytes(header[4..8].try_into().expect("header has four bytes")) as usize;
    let val_len =
        u32::from_le_bytes(header[8..12].try_into().expect("header has four bytes")) as usize;
    let deleted = match header[12] {
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

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&header[4..]);
    hasher.update(&key[..]);
    hasher.update(&val[..]);
    if hasher.finalize() != checksum {
        return Err(DecodeError::ChecksumMismatch);
    }

    Ok(Some(Entry {
        key: Bytes::from(key),
        val: Bytes::from(val),
        deleted,
    }))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, ErrorKind};

    use super::*;

    // TODO: Add property tests for encode/decode round trips over arbitrary binary keys and values.
    // TODO: Add a fuzz target to ensure malformed frames never panic the decoder.

    #[test]
    fn entry_encode() -> Result<(), EncodeError> {
        let entry = Entry::new(Bytes::from("hello"), Bytes::from("world"), true);
        let encoded = entry.encode()?;

        assert_eq!(
            &encoded[CHECKSUM_SIZE..],
            b"\x05\x00\x00\x00\x05\x00\x00\x00\x01helloworld"
        );
        assert_eq!(
            u32::from_le_bytes(encoded[..CHECKSUM_SIZE].try_into().unwrap()),
            crc32fast::hash(&encoded[CHECKSUM_SIZE..])
        );
        Ok(())
    }

    #[test]
    fn entry_decode_from_reader_returns_entry_then_eof() -> Result<(), DecodeError> {
        let entry = Entry::new(
            Bytes::from_static(b"key"),
            Bytes::from_static(b"value"),
            true,
        );
        let mut reader = Cursor::new(entry.encode().expect("test entry fits in the format"));

        let decoded = decode(&mut reader)?.expect("entry should be present");

        assert_eq!(decoded, entry);
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
        let mut bytes = vec![0; CHECKSUM_SIZE];
        bytes.extend_from_slice(&((MAX_ENTRY_SIZE + 1) as u32).to_le_bytes());
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
    fn entry_decode_from_reader_round_trips_long_binary_data() -> Result<(), DecodeError> {
        let entry = Entry::new(
            Bytes::from((0..=255).collect::<Vec<_>>()),
            Bytes::from((0..=255).rev().cycle().take(1_024).collect::<Vec<_>>()),
            true,
        );
        let mut reader = Cursor::new(entry.encode().expect("test entry fits in the format"));

        assert_eq!(decode(&mut reader)?, Some(entry));
        Ok(())
    }

    #[test]
    fn entry_decode_from_reader_consumes_only_one_entry() -> Result<(), DecodeError> {
        let first = Entry::new(
            Bytes::from_static(b"first"),
            Bytes::from_static(b"value"),
            false,
        );
        let second = Entry::new(
            Bytes::from_static(b"second"),
            Bytes::from_static(b"tombstone"),
            true,
        );
        let first_encoded = first.encode().expect("test entry fits in the format");
        let second_encoded = second.encode().expect("test entry fits in the format");
        let mut bytes = first_encoded.to_vec();
        bytes.extend_from_slice(&second_encoded);
        let mut reader = Cursor::new(bytes);

        assert_eq!(decode(&mut reader)?, Some(first));
        assert_eq!(
            reader.get_ref()[reader.position() as usize..],
            second_encoded
        );
        assert_eq!(decode(&mut reader)?, Some(second));
        assert!(decode(&mut reader)?.is_none());
        Ok(())
    }

    #[test]
    fn entry_decode_from_reader_rejects_payload_checksum_mismatch() {
        let entry = Entry::new(
            Bytes::from_static(b"key"),
            Bytes::from_static(b"value"),
            false,
        );
        let mut encoded = entry.encode().expect("test entry fits in the format");
        let last = encoded.len() - 1;
        encoded[last] ^= 1;

        let error = decode(&mut Cursor::new(encoded)).unwrap_err();
        assert!(matches!(error, DecodeError::ChecksumMismatch));
    }

    #[test]
    fn entry_decode_from_reader_rejects_header_checksum_mismatch() {
        let entry = Entry::new(Bytes::from_static(b"key"), Bytes::new(), true);
        let mut encoded = entry.encode().expect("test entry fits in the format");
        encoded[HEADER_SIZE - 1] ^= 1;

        let error = decode(&mut Cursor::new(encoded)).unwrap_err();
        assert!(matches!(error, DecodeError::ChecksumMismatch));
    }

    #[test]
    fn entry_decode_from_reader_rejects_truncated_payload() {
        let entry = Entry::new(
            Bytes::from_static(b"key"),
            Bytes::from_static(b"value"),
            false,
        );
        let mut encoded = entry.encode().expect("test entry fits in the format");
        encoded.truncate(encoded.len() - 1);

        let error = decode(&mut Cursor::new(encoded)).unwrap_err();
        assert!(matches!(
            error,
            DecodeError::ReadValue(source) if source.kind() == ErrorKind::UnexpectedEof
        ));
    }
}
