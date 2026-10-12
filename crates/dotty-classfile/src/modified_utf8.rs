use crate::encode_error::EncodeError;

/// Encodes a Rust string as the JVM modified UTF-8 payload used by
/// `CONSTANT_Utf8_info` (JVMS §4.4.7).
///
/// This returns only the encoded payload, without its `u2` length prefix or a
/// constant-pool tag. The encoded byte length must fit in a `u2`; overflow is
/// reported before any result is returned.
///
/// Rust strings contain Unicode scalar values and therefore cannot represent
/// unpaired UTF-16 surrogates. Such raw UTF-16 payloads cannot be round-tripped
/// through this API and remain rejected by the existing reader.
pub fn encode_modified_utf8(value: &str) -> Result<Vec<u8>, EncodeError> {
    let mut bytes = Vec::new();

    for character in value.chars() {
        let mut utf16_units = [0; 2];
        for unit in character.encode_utf16(&mut utf16_units).iter().copied() {
            let encoded = if (1..=0x7F).contains(&unit) {
                [unit as u8, 0, 0]
            } else if unit <= 0x7FF {
                [0xC0 | ((unit >> 6) as u8), 0x80 | ((unit & 0x3F) as u8), 0]
            } else {
                [
                    0xE0 | ((unit >> 12) as u8),
                    0x80 | (((unit >> 6) & 0x3F) as u8),
                    0x80 | ((unit & 0x3F) as u8),
                ]
            };
            let encoded_len = if unit <= 0x7F && unit != 0 {
                1
            } else if unit <= 0x7FF {
                2
            } else {
                3
            };
            let byte_len = bytes.len() + encoded_len;
            if byte_len > usize::from(u16::MAX) {
                return Err(EncodeError::Utf8LengthOverflow {
                    byte_len,
                    max: u16::MAX,
                });
            }
            bytes.extend_from_slice(&encoded[..encoded_len]);
        }
    }

    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use crate::classfile::{EncodeError, Writer, encode_modified_utf8};
    use crate::reader::Reader;

    #[test]
    fn encodes_modified_utf8_code_unit_boundaries() {
        assert_eq!(encode_modified_utf8("\u{1}~"), Ok(vec![0x01, 0x7E]));
        assert_eq!(encode_modified_utf8("\u{7F}"), Ok(vec![0x7F]));
        assert_eq!(encode_modified_utf8("\u{80}"), Ok(vec![0xC2, 0x80]));
        assert_eq!(encode_modified_utf8("\u{800}"), Ok(vec![0xE0, 0xA0, 0x80]));
        assert_eq!(encode_modified_utf8("\u{FFFF}"), Ok(vec![0xEF, 0xBF, 0xBF]));
    }

    #[test]
    fn encodes_nul_and_supplementary_characters_as_exact_wire_bytes() {
        assert_eq!(encode_modified_utf8("\0"), Ok(vec![0xC0, 0x80]));
        assert_eq!(
            encode_modified_utf8("😀"),
            Ok(vec![0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80])
        );
    }

    #[test]
    fn modified_utf8_encoder_roundtrips_through_the_existing_reader() {
        for value in ["", "Scala 3.9.0", "a\0b", "é水", "😀", "\u{1}\u{7F}\u{80}"] {
            let encoded = encode_modified_utf8(value).unwrap();
            assert_eq!(encoded, encode_modified_utf8(value).unwrap());
            let mut reader = Reader::new(&encoded);
            assert_eq!(reader.read_modified_utf8(encoded.len()).unwrap(), value);
            assert!(reader.is_at_end());
        }
    }

    #[test]
    fn accepts_the_maximum_multibyte_payload_length() {
        let value = "\u{800}".repeat(usize::from(u16::MAX) / 3);
        let encoded = encode_modified_utf8(&value).unwrap();
        assert_eq!(encoded.len(), usize::from(u16::MAX));
    }

    #[test]
    fn accepts_and_rejects_ascii_payloads_at_the_u2_boundary() {
        let maximum = "a".repeat(usize::from(u16::MAX));
        assert_eq!(
            encode_modified_utf8(&maximum).unwrap().len(),
            usize::from(u16::MAX)
        );

        let too_long = "a".repeat(usize::from(u16::MAX) + 1);
        assert_eq!(
            encode_modified_utf8(&too_long),
            Err(EncodeError::Utf8LengthOverflow {
                byte_len: usize::from(u16::MAX) + 1,
                max: u16::MAX,
            })
        );
    }

    #[test]
    fn rejects_multibyte_length_overflow_with_the_encoded_byte_count() {
        let value = "\u{800}".repeat(usize::from(u16::MAX) / 3 + 1);
        assert_eq!(
            encode_modified_utf8(&value),
            Err(EncodeError::Utf8LengthOverflow {
                byte_len: usize::from(u16::MAX) + 3,
                max: u16::MAX,
            })
        );
    }

    #[test]
    fn writer_writes_a_checked_length_and_payload_and_is_unchanged_on_overflow() {
        let mut writer = Writer::new();
        writer.write_modified_utf8("\0😀").unwrap();
        assert_eq!(
            writer.clone().into_bytes(),
            [0, 8, 0xC0, 0x80, 0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]
        );

        let before = writer.clone();
        let too_long = "\u{800}".repeat(usize::from(u16::MAX) / 3 + 1);
        assert_eq!(
            writer.write_modified_utf8(&too_long),
            Err(EncodeError::Utf8LengthOverflow {
                byte_len: usize::from(u16::MAX) + 3,
                max: u16::MAX,
            })
        );
        assert_eq!(writer, before);
    }
}
