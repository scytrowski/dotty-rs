use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    LengthOverflow { length: usize },
}

impl fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LengthOverflow { length } => {
                write!(formatter, "length {length} does not fit in a TASTy Nat")
            }
        }
    }
}

impl std::error::Error for WriteError {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    pub fn position(&self) -> usize {
        self.bytes.len()
    }

    pub fn write_u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    pub fn write_nat(&mut self, value: u32) {
        write_unsigned(&mut self.bytes, u64::from(value));
    }

    pub fn write_long_nat(&mut self, value: u64) {
        write_unsigned(&mut self.bytes, value);
    }

    pub fn write_int(&mut self, value: i32) {
        write_signed(&mut self.bytes, i64::from(value));
    }

    pub fn write_long_int(&mut self, value: i64) {
        write_signed(&mut self.bytes, value);
    }

    pub fn write_utf8(&mut self, value: &str) -> Result<(), WriteError> {
        self.write_length_prefixed_bytes(value.as_bytes())
    }

    pub fn write_length_prefixed_bytes(&mut self, bytes: &[u8]) -> Result<(), WriteError> {
        let length = u32::try_from(bytes.len()).map_err(|_| WriteError::LengthOverflow {
            length: bytes.len(),
        })?;
        self.write_nat(length);
        self.write_bytes(bytes);
        Ok(())
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.bytes
    }

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
    fn writes_length_prefixed_utf8() {
        let mut writer = Writer::new();
        writer.write_utf8("zażółć").unwrap();
        let bytes = writer.into_inner();
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
