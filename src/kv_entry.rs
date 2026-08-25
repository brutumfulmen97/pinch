use bytes::{Buf, BufMut, Bytes, BytesMut, TryGetError};

#[derive(Debug)]
pub(crate) struct Entry {
    key: Bytes,
    val: Bytes,
}

#[derive(Debug)]
pub(crate) enum EncodeError {
    KeyTooLarge { length: usize },
    ValueTooLarge { length: usize },
}

impl Entry {
    fn encode(&self) -> Result<BytesMut, EncodeError> {
        let key_len = self.key.len();
        let val_len = self.val.len();

        if key_len > u32::MAX as usize {
            return Err(EncodeError::KeyTooLarge { length: key_len });
        }

        if val_len > u32::MAX as usize {
            return Err(EncodeError::ValueTooLarge { length: val_len });
        }

        let mut buf = BytesMut::with_capacity(4 + 4 + key_len + val_len);
        buf.put_u32_le(key_len as u32);
        buf.put_u32_le(val_len as u32);
        buf.put_slice(&self.key);
        buf.put_slice(&self.val);
        Ok(buf)
    }

    fn decode(&self, bytes: &mut Bytes) -> Result<Entry, TryGetError> {
        let key_len = bytes.try_get_u32_le()? as usize;
        let val_len = bytes.try_get_u32_le()? as usize;

        if bytes.remaining() < key_len + val_len {
            return Err(TryGetError {
                requested: key_len + val_len,
                available: bytes.remaining(),
            });
        }
        let key = bytes.copy_to_bytes(key_len);
        let val = bytes.copy_to_bytes(val_len);
        Ok(Entry { key, val })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_encode() -> Result<(), EncodeError> {
        let entry = Entry {
            key: Bytes::from("hello"),
            val: Bytes::from("world"),
        };
        let encoded = entry.encode()?;
        assert_eq!(
            encoded.as_ref(),
            b"\x05\x00\x00\x00\x05\x00\x00\x00helloworld"
        );
        Ok(())
    }

    #[test]
    fn entry_decode() -> Result<(), TryGetError> {
        let bytes = b"\x05\x00\x00\x00\x05\x00\x00\x00helloworld";

        let entry = Entry {
            key: Bytes::default(),
            val: Bytes::default(),
        };
        let decoded = entry.decode(&mut Bytes::from_static(bytes))?;
        assert_eq!(decoded.key, Bytes::from_static(b"hello"));
        assert_eq!(decoded.val, Bytes::from_static(b"world"));
        Ok(())
    }
}
