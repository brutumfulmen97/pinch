use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::cell::{self, Cell, CellType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Schema {
    table: String,
    columns: Vec<Column>,
    primary_key: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Column {
    name: String,
    cell_type: CellType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row(Vec<Cell>);

#[derive(Debug, thiserror::Error)]
pub(crate) enum SchemaError {
    #[error("primary-key column index {index} is outside the {column_count}-column schema")]
    PrimaryKeyIndexOutOfBounds { index: usize, column_count: usize },
    #[error("column {index} appears more than once in the primary key")]
    DuplicatePrimaryKeyColumn { index: usize },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RowError {
    #[error("column count mismatch: expected {expected}, got {actual}")]
    ColumnCountMismatch { expected: usize, actual: usize },
    #[error("cell type mismatch at column {index}: expected {expected}, got {actual}")]
    CellTypeMismatch {
        index: usize,
        expected: CellType,
        actual: CellType,
    },
    #[error(
        "key is shorter than the table prefix: expected at least {expected} bytes, got {actual}"
    )]
    TruncatedKeyPrefix { expected: usize, actual: usize },
    #[error("key belongs to a different table")]
    TableMismatch,
    #[error("key has {remaining} trailing bytes after decoding primary-key columns")]
    TrailingKeyBytes { remaining: usize },
    #[error("value has {remaining} trailing bytes after decoding non-primary-key columns")]
    TrailingValueBytes { remaining: usize },
    #[error("column {index} was not decoded")]
    MissingCell { index: usize },
    #[error("failed to decode row cell")]
    CellDecode(#[from] cell::DecodeError),
    #[error("failed to encode row cell")]
    CellEncode(#[from] cell::EncodeError),
}

impl Column {
    pub(crate) fn new(name: impl Into<String>, cell_type: CellType) -> Self {
        Self {
            name: name.into(),
            cell_type,
        }
    }
}

impl Schema {
    pub(crate) fn new(
        table: impl Into<String>,
        columns: Vec<Column>,
        primary_key: Vec<usize>,
    ) -> Result<Self, SchemaError> {
        for (position, &index) in primary_key.iter().enumerate() {
            if index >= columns.len() {
                return Err(SchemaError::PrimaryKeyIndexOutOfBounds {
                    index,
                    column_count: columns.len(),
                });
            }
            if primary_key[..position].contains(&index) {
                return Err(SchemaError::DuplicatePrimaryKeyColumn { index });
            }
        }

        Ok(Self {
            table: table.into(),
            columns,
            primary_key,
        })
    }

    fn is_primary_key(&self, index: usize) -> bool {
        self.primary_key.contains(&index)
    }
}

impl Row {
    pub(crate) fn new(cells: Vec<Cell>) -> Self {
        Self(cells)
    }

    pub(crate) fn encode_key(&self, schema: &Schema) -> Result<BytesMut, RowError> {
        self.validate(schema)?;

        let mut key = BytesMut::with_capacity(schema.table.len() + 1);
        key.put_slice(schema.table.as_bytes());
        key.put_u8(0);
        for (index, cell) in self.0.iter().enumerate() {
            if schema.is_primary_key(index) {
                cell.encode_into(&mut key)?;
            }
        }
        Ok(key)
    }

    pub(crate) fn encode_value(&self, schema: &Schema) -> Result<BytesMut, RowError> {
        self.validate(schema)?;

        let mut value = BytesMut::new();
        for (index, cell) in self.0.iter().enumerate() {
            if !schema.is_primary_key(index) {
                cell.encode_into(&mut value)?;
            }
        }
        Ok(value)
    }

    pub(crate) fn decode(
        schema: &Schema,
        key: &mut Bytes,
        value: &mut Bytes,
    ) -> Result<Self, RowError> {
        let mut cells = std::iter::repeat_with(|| None)
            .take(schema.columns.len())
            .collect::<Vec<Option<Cell>>>();

        Self::decode_key(schema, key, &mut cells)?;
        Self::decode_value(schema, value, &mut cells)?;

        let cells = cells
            .into_iter()
            .enumerate()
            .map(|(index, cell)| cell.ok_or(RowError::MissingCell { index }))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(cells))
    }

    fn validate(&self, schema: &Schema) -> Result<(), RowError> {
        if self.0.len() != schema.columns.len() {
            return Err(RowError::ColumnCountMismatch {
                expected: schema.columns.len(),
                actual: self.0.len(),
            });
        }

        for (index, cell) in self.0.iter().enumerate() {
            let expected = schema.columns[index].cell_type;
            let actual = cell.cell_type();
            if expected != actual {
                return Err(RowError::CellTypeMismatch {
                    index,
                    expected,
                    actual,
                });
            }
        }
        Ok(())
    }

    fn decode_key(
        schema: &Schema,
        key: &mut Bytes,
        cells: &mut [Option<Cell>],
    ) -> Result<(), RowError> {
        let prefix_len = schema.table.len() + 1;
        if key.remaining() < prefix_len {
            return Err(RowError::TruncatedKeyPrefix {
                expected: prefix_len,
                actual: key.remaining(),
            });
        }
        if &key[..schema.table.len()] != schema.table.as_bytes() || key[schema.table.len()] != 0 {
            return Err(RowError::TableMismatch);
        }
        key.advance(prefix_len);

        for (index, column) in schema.columns.iter().enumerate() {
            if schema.is_primary_key(index) {
                cells[index] = Some(Cell::decode(column.cell_type, key)?);
            }
        }
        if key.has_remaining() {
            return Err(RowError::TrailingKeyBytes {
                remaining: key.remaining(),
            });
        }
        Ok(())
    }

    fn decode_value(
        schema: &Schema,
        value: &mut Bytes,
        cells: &mut [Option<Cell>],
    ) -> Result<(), RowError> {
        for (index, column) in schema.columns.iter().enumerate() {
            if !schema.is_primary_key(index) {
                cells[index] = Some(Cell::decode(column.cell_type, value)?);
            }
        }
        if value.has_remaining() {
            return Err(RowError::TrailingValueBytes {
                remaining: value.remaining(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> Schema {
        Schema::new(
            "link",
            vec![
                Column::new("time", CellType::I64),
                Column::new("src", CellType::Str),
                Column::new("dst", CellType::Str),
            ],
            vec![1, 2],
        )
        .expect("test schema is valid")
    }

    fn row() -> Row {
        Row::new(vec![
            Cell::I64(123),
            Cell::Str(Bytes::from_static(b"a")),
            Cell::Str(Bytes::from_static(b"b")),
        ])
    }

    #[test]
    fn row_encodes_and_decodes_key_and_value() -> Result<(), RowError> {
        let schema = schema();
        let row = row();
        let mut key = row.encode_key(&schema)?.freeze();
        let mut value = row.encode_value(&schema)?.freeze();

        assert_eq!(key.as_ref(), b"link\0\x01\x00\x00\x00a\x01\x00\x00\x00b");
        assert_eq!(value.as_ref(), b"{\0\0\0\0\0\0\0");
        assert_eq!(Row::decode(&schema, &mut key, &mut value)?, row);
        assert!(key.is_empty());
        assert!(value.is_empty());
        Ok(())
    }

    #[test]
    fn row_rejects_cell_type_mismatch() {
        let row = Row::new(vec![
            Cell::Str(Bytes::from_static(b"not an integer")),
            Cell::Str(Bytes::from_static(b"a")),
            Cell::Str(Bytes::from_static(b"b")),
        ]);

        let error = row.encode_key(&schema()).unwrap_err();
        assert!(matches!(
            error,
            RowError::CellTypeMismatch {
                index: 0,
                expected: CellType::I64,
                actual: CellType::Str,
            }
        ));
    }

    #[test]
    fn row_rejects_key_for_another_table() {
        let schema = schema();
        let mut key = Bytes::from_static(b"other\0\x01\x00\x00\x00a\x01\x00\x00\x00b");
        let mut value = Bytes::from_static(b"{\0\0\0\0\0\0\0");

        let error = Row::decode(&schema, &mut key, &mut value).unwrap_err();
        assert!(matches!(error, RowError::TableMismatch));
    }

    #[test]
    fn schema_rejects_invalid_primary_key_columns() {
        let columns = vec![Column::new("id", CellType::I64)];

        let out_of_bounds = Schema::new("table", columns.clone(), vec![1]).unwrap_err();
        assert!(matches!(
            out_of_bounds,
            SchemaError::PrimaryKeyIndexOutOfBounds {
                index: 1,
                column_count: 1,
            }
        ));

        let duplicate = Schema::new("table", columns, vec![0, 0]).unwrap_err();
        assert!(matches!(
            duplicate,
            SchemaError::DuplicatePrimaryKeyColumn { index: 0 }
        ));
    }
}
