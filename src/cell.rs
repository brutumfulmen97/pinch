use bytes::{Buf, BufMut, Bytes, BytesMut};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CellType {
    I64,
    Str,
}

impl std::fmt::Display for CellType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::I64 => write!(f, "I64"),
            Self::Str => write!(f, "Str"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum EncodeError {
    #[error("string length {length} exceeds the maximum encodable length")]
    StringTooLarge { length: usize },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum DecodeError {
    #[error("i64 cell payload is truncated")]
    TruncatedI64,
    #[error("string cell length is truncated")]
    TruncatedStringLength,
    #[error("string cell payload is truncated: expected {expected} bytes, only {available} remain")]
    TruncatedString { expected: usize, available: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cell {
    I64(i64),
    Str(Bytes),
}

impl Cell {
    pub(crate) fn encode_into(&self, out: &mut BytesMut) -> Result<(), EncodeError> {
        match self {
            Self::I64(value) => {
                out.put_i64_le(*value);
            }
            Self::Str(value) => {
                let length =
                    u32::try_from(value.len()).map_err(|_| EncodeError::StringTooLarge {
                        length: value.len(),
                    })?;
                out.put_u32_le(length);
                out.put_slice(value);
            }
        }

        Ok(())
    }

    pub(crate) const fn cell_type(&self) -> CellType {
        match self {
            Self::I64(_) => CellType::I64,
            Self::Str(_) => CellType::Str,
        }
    }

    pub(crate) fn decode(cell_type: CellType, input: &mut Bytes) -> Result<Self, DecodeError> {
        match cell_type {
            CellType::I64 => {
                if input.remaining() < 8 {
                    return Err(DecodeError::TruncatedI64);
                }
                Ok(Self::I64(input.get_i64_le()))
            }
            CellType::Str => {
                if input.remaining() < 4 {
                    return Err(DecodeError::TruncatedStringLength);
                }

                let length = input.get_u32_le() as usize;
                if input.remaining() < length {
                    return Err(DecodeError::TruncatedString {
                        expected: length,
                        available: input.remaining(),
                    });
                }

                Ok(Self::Str(input.split_to(length)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_encodes_i64() -> Result<(), EncodeError> {
        let mut encoded = BytesMut::new();

        Cell::I64(-42).encode_into(&mut encoded)?;

        assert_eq!(encoded.as_ref(), b"\xd6\xff\xff\xff\xff\xff\xff\xff");
        Ok(())
    }

    #[test]
    fn cell_encodes_and_decodes_a_string() -> Result<(), DecodeError> {
        let cell = Cell::Str(Bytes::from_static(b"hello"));
        let mut encoded = BytesMut::new();
        cell.encode_into(&mut encoded).expect("test string fits");
        let mut input = encoded.freeze();

        assert_eq!(Cell::decode(CellType::Str, &mut input)?, cell);
        assert!(input.is_empty());
        Ok(())
    }

    #[test]
    fn cell_decode_consumes_one_cell() -> Result<(), DecodeError> {
        let mut encoded = BytesMut::new();
        Cell::I64(7)
            .encode_into(&mut encoded)
            .expect("i64 always fits");
        Cell::Str(Bytes::from_static(b"next"))
            .encode_into(&mut encoded)
            .expect("test string fits");
        let mut input = encoded.freeze();

        assert_eq!(Cell::decode(CellType::I64, &mut input)?, Cell::I64(7));
        assert_eq!(
            Cell::decode(CellType::Str, &mut input)?,
            Cell::Str(Bytes::from_static(b"next"))
        );
        assert!(input.is_empty());
        Ok(())
    }

    #[test]
    fn cell_decode_rejects_truncated_i64() {
        let error = Cell::decode(CellType::I64, &mut Bytes::from_static(b"\xff")).unwrap_err();

        assert!(matches!(error, DecodeError::TruncatedI64));
    }

    #[test]
    fn cell_decode_rejects_truncated_string_payload() {
        let bytes = b"\x05\x00\x00\x00abc";
        let error = Cell::decode(CellType::Str, &mut Bytes::from_static(bytes)).unwrap_err();

        assert!(matches!(
            error,
            DecodeError::TruncatedString {
                expected: 5,
                available: 3,
            }
        ));
    }
}
