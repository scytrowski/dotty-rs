use flate2::{Decompress, FlushDecompress, Status};
use std::fmt;

/// Errors decompressing a raw DEFLATE (RFC 1951) stream from a ZIP entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InflateError {
    /// The compressed input ended before the DEFLATE stream completed.
    UnexpectedEof,
    /// The DEFLATE decoder rejected the stream as malformed.
    InvalidData,
    /// Decompressed output exceeded the caller-supplied limit.
    OutputTooLarge { limit: usize },
}

impl fmt::Display for InflateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(formatter, "unexpected end of DEFLATE stream"),
            Self::InvalidData => write!(formatter, "invalid DEFLATE stream"),
            Self::OutputTooLarge { limit } => write!(
                formatter,
                "decompressed output exceeded the {limit}-byte limit"
            ),
        }
    }
}

impl std::error::Error for InflateError {}

/// Decompresses a raw DEFLATE (RFC 1951) stream — ZIP method 8 has no zlib
/// or gzip framing around it.
///
/// Output is decoded in bounded chunks. The decoder is allowed to produce at
/// most one byte beyond `max_output_len`, which is enough to detect an
/// over-limit stream without allocating its full output. This check is
/// incremental and does not rely on the size declared in the ZIP directory.
pub(crate) fn inflate(compressed: &[u8], max_output_len: usize) -> Result<Vec<u8>, InflateError> {
    const CHUNK_SIZE: usize = 8 * 1024;

    let mut decoder = Decompress::new(false);
    let mut output = Vec::new();
    let mut input_offset = 0usize;
    let mut chunk = [0u8; CHUNK_SIZE];

    loop {
        let remaining_output = max_output_len - output.len();
        let chunk_len = CHUNK_SIZE.min(remaining_output.saturating_add(1));
        let input = &compressed[input_offset..];
        let input_before = decoder.total_in();
        let output_before = decoder.total_out();
        let status = decoder
            .decompress(input, &mut chunk[..chunk_len], FlushDecompress::None)
            .map_err(|_| InflateError::InvalidData)?;
        let consumed = (decoder.total_in() - input_before) as usize;
        let produced = (decoder.total_out() - output_before) as usize;

        if produced > remaining_output {
            return Err(InflateError::OutputTooLarge {
                limit: max_output_len,
            });
        }

        output.extend_from_slice(&chunk[..produced]);
        input_offset += consumed;

        if status == Status::StreamEnd {
            return Ok(output);
        }

        if consumed == 0 && produced == 0 {
            if input_offset == compressed.len() {
                return Err(InflateError::UnexpectedEof);
            }
            // No progress with input still available would otherwise make the
            // loop unbounded. Treat it as malformed compressed data.
            return Err(InflateError::InvalidData);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BitWriter {
        bytes: Vec<u8>,
        bit_pos: u8,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: Vec::new(),
                bit_pos: 0,
            }
        }

        fn write_bit(&mut self, bit: u8) {
            if self.bit_pos == 0 {
                self.bytes.push(0);
            }
            let last = self.bytes.len() - 1;
            self.bytes[last] |= (bit & 1) << self.bit_pos;
            self.bit_pos = (self.bit_pos + 1) % 8;
        }

        fn write_huffman_code(&mut self, value: u16, length: u8) {
            for bit in (0..length).rev() {
                self.write_bit(((value >> bit) & 1) as u8);
            }
        }

        fn write_bits(&mut self, value: u32, count: u8) {
            for bit in 0..count {
                self.write_bit(((value >> bit) & 1) as u8);
            }
        }

        fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }

    fn fixed_huffman_stream_with_back_reference() -> Vec<u8> {
        let mut writer = BitWriter::new();
        writer.write_bits(1, 1); // BFINAL = 1
        writer.write_bits(0b01, 2); // BTYPE = fixed Huffman
        writer.write_huffman_code(0x30 + u16::from(b'A'), 8);
        writer.write_huffman_code(0x30 + u16::from(b'B'), 8);
        writer.write_huffman_code(2, 7); // length 4
        writer.write_huffman_code(1, 5); // distance 2
        writer.write_huffman_code(0, 7); // end of block
        writer.finish()
    }

    #[test]
    fn decompresses_fixed_huffman_literals_and_back_references() {
        let compressed = fixed_huffman_stream_with_back_reference();

        assert_eq!(inflate(&compressed, 6).unwrap(), b"ABABAB");
    }

    #[test]
    fn accepts_an_empty_stream_at_a_zero_output_limit() {
        assert_eq!(inflate(&[0x03, 0x00], 0).unwrap(), b"");
    }

    #[test]
    fn rejects_output_beyond_the_limit_incrementally() {
        let compressed = fixed_huffman_stream_with_back_reference();

        assert_eq!(
            inflate(&compressed, 3),
            Err(InflateError::OutputTooLarge { limit: 3 })
        );
    }

    #[test]
    fn rejects_a_truncated_stream() {
        assert_eq!(
            inflate(&[0x03], usize::MAX),
            Err(InflateError::UnexpectedEof)
        );
    }

    #[test]
    fn rejects_a_reserved_block_type() {
        assert_eq!(inflate(&[0x07], usize::MAX), Err(InflateError::InvalidData));
    }
}
