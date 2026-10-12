use crate::encode_error::{EncodeError, LengthWidth};

/// An owned append-only writer for fixed-width, big-endian class-file values.
///
/// This writes binary fields; it does not check JVM verification rules or
/// serialize a class-file model. Checked length-prefix methods validate the
/// value before appending any bytes, so their failure leaves this writer
/// unchanged. Higher-level encoders should validate their model before using
/// this writer and only expose completed output on success.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// Creates an empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a JVM `u1` value.
    pub fn write_u1(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// Appends a JVM `u2` value in big-endian byte order.
    pub fn write_u2(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// Appends a JVM `u4` value in big-endian byte order.
    pub fn write_u4(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// Appends raw bytes without interpreting their contents.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Checks and appends a value as a JVM `u2` count or length prefix.
    ///
    /// If `length` does not fit in `u16`, this returns [`EncodeError`] without
    /// changing the writer.
    pub fn write_u2_length(&mut self, length: usize) -> Result<(), EncodeError> {
        let value = checked_u16_len(length)?;
        self.write_u2(value);
        Ok(())
    }

    /// Checks and appends a value as a JVM `u4` count or length prefix.
    ///
    /// If `length` does not fit in `u32`, this returns [`EncodeError`] without
    /// changing the writer.
    pub fn write_u4_length(&mut self, length: usize) -> Result<(), EncodeError> {
        let value = checked_u32_len(length)?;
        self.write_u4(value);
        Ok(())
    }

    /// Returns the number of bytes written so far.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether no bytes have been written.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Consumes the writer and returns its bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Converts a count or length to `u16`, returning a typed error on overflow.
pub fn checked_u16_len(value: usize) -> Result<u16, EncodeError> {
    u16::try_from(value).map_err(|_| EncodeError::CountOverflow {
        value,
        width: LengthWidth::U16,
    })
}

/// Converts a count or length to `u32`, returning a typed error on overflow.
pub fn checked_u32_len(value: usize) -> Result<u32, EncodeError> {
    u32::try_from(value).map_err(|_| EncodeError::CountOverflow {
        value,
        width: LengthWidth::U32,
    })
}

/// Converts a UTF-8 byte length to the JVM `u2` field used for UTF-8 entries.
///
/// This checks the supplied byte length only; it does not encode modified
/// UTF-8 or validate how the bytes were produced.
pub fn checked_utf8_len(byte_len: usize) -> Result<u16, EncodeError> {
    u16::try_from(byte_len).map_err(|_| EncodeError::Utf8LengthOverflow {
        byte_len,
        max: u16::MAX,
    })
}

#[cfg(test)]
mod tests {
    use super::{Writer, checked_u16_len, checked_u32_len, checked_utf8_len};
    use crate::encode_error::{EncodeError, LengthWidth};

    #[test]
    fn writes_integer_widths_in_big_endian_order_and_appends_raw_bytes() {
        let mut writer = Writer::new();
        writer.write_u1(0xAB);
        writer.write_u2(0x1234);
        writer.write_u4(0x5678_9ABC);
        writer.write_bytes(&[0xDE, 0xF0]);

        assert_eq!(
            writer.into_bytes(),
            [0xAB, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]
        );
    }

    #[test]
    fn empty_writer_produces_empty_bytes() {
        assert_eq!(Writer::new().into_bytes(), Vec::<u8>::new());
    }

    #[test]
    fn length_conversions_accept_zero_and_maximum_values() {
        assert_eq!(checked_u16_len(0), Ok(0));
        assert_eq!(checked_u16_len(usize::from(u16::MAX)), Ok(u16::MAX));
        assert_eq!(checked_u32_len(0), Ok(0));
        assert_eq!(checked_u32_len(u32::MAX as usize), Ok(u32::MAX));
        assert_eq!(checked_utf8_len(0), Ok(0));
        assert_eq!(checked_utf8_len(usize::from(u16::MAX)), Ok(u16::MAX));
    }

    #[test]
    fn length_conversions_reject_overflow_without_allocating() {
        let too_long_u16 = usize::from(u16::MAX) + 1;
        assert_eq!(
            checked_u16_len(too_long_u16),
            Err(EncodeError::CountOverflow {
                value: too_long_u16,
                width: LengthWidth::U16,
            })
        );
        assert_eq!(
            checked_utf8_len(too_long_u16),
            Err(EncodeError::Utf8LengthOverflow {
                byte_len: too_long_u16,
                max: u16::MAX,
            })
        );

        if let Some(too_long_u32) = (u32::MAX as usize).checked_add(1) {
            assert_eq!(
                checked_u32_len(too_long_u32),
                Err(EncodeError::CountOverflow {
                    value: too_long_u32,
                    width: LengthWidth::U32,
                })
            );
        }
    }

    #[test]
    fn failed_length_prefix_does_not_change_writer_and_repeated_output_is_stable() {
        let mut writer = Writer::new();
        assert!(writer.is_empty());
        writer.write_u1(0x7F);
        let before = writer.clone();
        let too_long = usize::from(u16::MAX) + 1;

        assert_eq!(
            writer.write_u2_length(too_long),
            Err(EncodeError::CountOverflow {
                value: too_long,
                width: LengthWidth::U16,
            })
        );
        assert_eq!(writer, before);
        assert_eq!(writer.len(), 1);

        let first = writer.clone().into_bytes();
        let second = writer.into_bytes();
        assert_eq!(first, [0x7F]);
        assert_eq!(first, second);
    }

    #[test]
    fn writes_zero_and_maximum_length_prefixes() {
        let mut writer = Writer::new();
        writer.write_u2_length(0).unwrap();
        writer.write_u2_length(usize::from(u16::MAX)).unwrap();
        writer.write_u4_length(0).unwrap();
        writer.write_u4_length(u32::MAX as usize).unwrap();

        assert_eq!(
            writer.into_bytes(),
            [0, 0, 0xFF, 0xFF, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]
        );
    }
}
