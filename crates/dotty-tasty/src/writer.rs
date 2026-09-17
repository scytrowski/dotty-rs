use std::collections::BTreeMap;
use std::fmt;

/// Errors returned while encoding TASTy values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// A length-prefixed payload is too large for the TASTy length encoding.
    LengthOverflow {
        /// Length that cannot be represented on the wire.
        length: usize,
    },
    /// A natural number is too large for the selected wire representation.
    NatOverflow {
        /// Value that cannot be represented as a TASTy natural number.
        value: u64,
    },
    /// A signed integer is outside the supported wire range.
    IntOverflow {
        /// Value that cannot be represented as a TASTy integer.
        value: i64,
    },
    /// The caller supplied a tag that is not encodable in the current context.
    InvalidTag {
        /// Invalid tag.
        tag: u8,
    },
    /// Attributes must be emitted in nondecreasing tag order.
    AttributeOrder {
        /// Previous attribute tag.
        previous: u8,
        /// Current attribute tag.
        current: u8,
    },
    /// A position header would be ambiguous with an AST address delta.
    PositionHeaderCollision {
        /// Address delta used by the association.
        address_delta: i64,
        /// Coordinate-presence flags.
        flags: u8,
    },
}

impl fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LengthOverflow { length } => {
                write!(formatter, "length {length} does not fit in a TASTy Nat")
            }
            Self::NatOverflow { value } => {
                write!(formatter, "value {value} does not fit in a TASTy Nat")
            }
            Self::IntOverflow { value } => {
                write!(formatter, "value {value} does not fit in a TASTy Int")
            }
            Self::InvalidTag { tag } => write!(formatter, "invalid TASTy tag {tag} for encoding"),
            Self::AttributeOrder { previous, current } => write!(
                formatter,
                "attribute tag {current} follows tag {previous}; attributes must be strictly ordered"
            ),
            Self::PositionHeaderCollision {
                address_delta,
                flags,
            } => write!(
                formatter,
                "position association ({address_delta}, flags {flags}) collides with the SOURCE entry header"
            ),
        }
    }
}

impl std::error::Error for WriteError {}

/// Growable output buffer for TASTy wire values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Writer {
    bytes: Vec<u8>,
    ast_address_map: Option<BTreeMap<u32, u32>>,
}

impl Writer {
    /// Creates an empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty writer with room for at least `capacity` bytes.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            ast_address_map: None,
        }
    }

    pub(crate) fn with_ast_address_map(map: BTreeMap<u32, u32>) -> Self {
        Self {
            bytes: Vec::new(),
            ast_address_map: Some(map),
        }
    }

    pub(crate) fn nested(&self) -> Self {
        Self {
            bytes: Vec::new(),
            ast_address_map: self.ast_address_map.clone(),
        }
    }

    pub(crate) fn has_ast_address_map(&self) -> bool {
        self.ast_address_map.is_some()
    }

    pub(crate) fn relocated_ast_ref(&self, value: u32) -> u32 {
        self.ast_address_map
            .as_ref()
            .and_then(|map| map.get(&value))
            .copied()
            .unwrap_or(value)
    }

    /// Returns the number of bytes currently written.
    pub fn position(&self) -> usize {
        self.bytes.len()
    }

    /// Appends one byte to the output.
    pub fn write_u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// Appends bytes to the output without adding a length prefix.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Writes a TASTy natural number.
    pub fn write_nat(&mut self, value: u32) {
        write_unsigned(&mut self.bytes, u64::from(value));
    }

    pub(crate) fn write_ast_ref(&mut self, value: u32) {
        self.write_nat(self.relocated_ast_ref(value));
    }

    /// Writes a 64-bit TASTy natural number.
    pub fn write_long_nat(&mut self, value: u64) {
        write_unsigned(&mut self.bytes, value);
    }

    /// Writes a signed TASTy integer.
    pub fn write_int(&mut self, value: i32) {
        write_signed(&mut self.bytes, i64::from(value));
    }

    /// Writes a signed 64-bit TASTy integer.
    pub fn write_long_int(&mut self, value: i64) {
        write_signed(&mut self.bytes, value);
    }

    /// Writes a length-prefixed UTF-8 string.
    pub fn write_utf8(&mut self, value: &str) -> Result<(), WriteError> {
        self.write_length_prefixed_bytes(value.as_bytes())
    }

    /// Writes a length-prefixed opaque byte payload.
    pub fn write_length_prefixed_bytes(&mut self, bytes: &[u8]) -> Result<(), WriteError> {
        let length = u32::try_from(bytes.len()).map_err(|_| WriteError::LengthOverflow {
            length: bytes.len(),
        })?;
        self.write_nat(length);
        self.write_bytes(bytes);
        Ok(())
    }

    /// Returns the owned encoded bytes and consumes the writer.
    pub fn into_inner(self) -> Vec<u8> {
        self.bytes
    }

    /// Borrows the encoded bytes written so far.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

fn write_unsigned(output: &mut Vec<u8>, value: u64) {
    let mut groups = [0u8; 10];
    let mut count = groups.len();
    let mut remaining = value;

    loop {
        count -= 1;
        groups[count] = (remaining & 0x7f) as u8;
        if remaining < 0x80 {
            break;
        }
        remaining >>= 7;
    }

    for (index, group) in groups[count..].iter().enumerate() {
        output.push(
            *group
                | if index + 1 == groups.len() - count {
                    0x80
                } else {
                    0
                },
        );
    }
}

fn write_signed(output: &mut Vec<u8>, value: i64) {
    let value = i128::from(value);
    let mut group_count = 1;

    while !fits_signed(value, group_count * 7) {
        group_count += 1;
    }

    let bits = group_count * 7;
    let raw = if value < 0 {
        (1i128 << bits) + value
    } else {
        value
    } as u128;

    let mut groups = [0u8; 10];
    let mut count = groups.len();
    let mut remaining = raw;
    for _ in 0..group_count {
        count -= 1;
        groups[count] = (remaining & 0x7f) as u8;
        remaining >>= 7;
    }

    for (index, group) in groups[count..].iter().enumerate() {
        output.push(*group | if index + 1 == group_count { 0x80 } else { 0 });
    }
}

fn fits_signed(value: i128, bits: usize) -> bool {
    let limit = 1i128 << (bits - 1);
    -limit <= value && value < limit
}

#[cfg(test)]
mod tests {
    use super::{WriteError, Writer};
    use crate::reader::Reader;

    #[test]
    fn writes_natural_numbers_that_reader_can_decode() {
        for value in [0, 1, 127, 128, 16_383, 16_384, u32::MAX] {
            let mut writer = Writer::new();
            writer.write_nat(value);
            let bytes = writer.into_inner();
            let mut reader = Reader::new(&bytes);

            assert_eq!(reader.read_nat().unwrap(), value);
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn writes_minimal_natural_number_boundaries() {
        let cases = [
            (0, vec![0x80]),
            (127, vec![0xff]),
            (128, vec![0x01, 0x80]),
            (16_383, vec![0x7f, 0xff]),
            (16_384, vec![0x01, 0x00, 0x80]),
            (u32::MAX, vec![0x0f, 0x7f, 0x7f, 0x7f, 0xff]),
        ];

        for (value, expected) in cases {
            let mut writer = Writer::new();
            writer.write_nat(value);
            assert_eq!(writer.into_inner(), expected, "value {value}");
        }
    }

    #[test]
    fn writes_signed_numbers_that_reader_can_decode() {
        for value in [
            i64::MIN,
            i64::MIN + 1,
            -16_385,
            -129,
            -128,
            -1,
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            i64::MAX - 1,
            i64::MAX,
        ] {
            let mut writer = Writer::new();
            writer.write_long_int(value);
            let bytes = writer.into_inner();
            let mut reader = Reader::new(&bytes);

            assert_eq!(reader.read_long_int().unwrap(), value, "value {value}");
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn writes_minimal_signed_integer_boundaries() {
        let cases = [
            (-64, vec![0xc0]),
            (63, vec![0xbf]),
            (64, vec![0x00, 0xc0]),
            (-65, vec![0x7f, 0xbf]),
        ];

        for (value, expected) in cases {
            let mut writer = Writer::new();
            writer.write_long_int(value);
            assert_eq!(writer.into_inner(), expected, "value {value}");
        }
    }

    #[test]
    fn writes_length_prefixed_utf8() {
        let mut writer = Writer::new();
        writer.write_utf8("zażółć").unwrap();
        let bytes = writer.into_inner();

        assert_eq!(bytes[0], 0x8a, "UTF-8 prefix must count bytes");
        let mut reader = Reader::new(&bytes);

        assert_eq!(reader.read_utf8().unwrap(), "zażółć");
        assert!(reader.is_at_end());
    }

    #[test]
    fn reports_length_overflow() {
        let error = WriteError::LengthOverflow { length: usize::MAX };
        assert_eq!(
            error.to_string(),
            format!("length {} does not fit in a TASTy Nat", usize::MAX)
        );
    }
}
