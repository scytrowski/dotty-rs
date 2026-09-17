use std::collections::HashMap;
use std::fmt;

/// Errors decompressing a DEFLATE (RFC 1951) stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InflateError {
    /// The bit stream ended before a value that was expected could be
    /// fully read.
    UnexpectedEof,
    /// While decoding a Huffman-coded value, no assigned code matched the
    /// bits read, even after reading the longest assigned code length.
    InvalidHuffmanCode,
}

impl fmt::Display for InflateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(formatter, "unexpected end of DEFLATE stream"),
            Self::InvalidHuffmanCode => write!(formatter, "invalid Huffman code in DEFLATE stream"),
        }
    }
}

impl std::error::Error for InflateError {}

/// A bit reader over a DEFLATE stream. Per RFC 1951 §3.1.1, non-Huffman
/// fields are packed least-significant-bit first within each byte;
/// [`BitReader::read_huffman_bit`] exists separately because Huffman
/// codes themselves are packed most-significant-bit first (§3.2.2) even
/// though the underlying bit order within each byte is unchanged.
pub(crate) struct BitReader<'a> {
    bytes: &'a [u8],
    byte_pos: usize,
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    fn read_bit(&mut self) -> Result<u32, InflateError> {
        let byte = *self
            .bytes
            .get(self.byte_pos)
            .ok_or(InflateError::UnexpectedEof)?;
        let bit = u32::from((byte >> self.bit_pos) & 1);

        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }

        Ok(bit)
    }

    /// Reads `count` bits from a non-Huffman field (extra bits, stored
    /// block lengths, dynamic-block header counts, ...): each new bit
    /// becomes the *next higher* bit of the result, per §3.1.1.
    pub(crate) fn read_bits(&mut self, count: u8) -> Result<u32, InflateError> {
        let mut value = 0u32;
        for i in 0..count {
            value |= self.read_bit()? << i;
        }
        Ok(value)
    }
}

/// A canonical Huffman decode table (RFC 1951 §3.2.2), built from an
/// array of per-symbol code lengths (`0` meaning "this symbol has no
/// code").
pub(crate) struct HuffmanTable {
    /// Keyed by `(code length, code value)`; the code value is the bits
    /// read so far, built most-significant-bit first as they arrive.
    codes: HashMap<(u8, u16), u16>,
    max_length: u8,
}

impl HuffmanTable {
    pub(crate) fn from_code_lengths(lengths: &[u8]) -> Self {
        let max_length = lengths.iter().copied().max().unwrap_or(0);
        if max_length == 0 {
            return Self {
                codes: HashMap::new(),
                max_length: 0,
            };
        }

        let mut bit_length_count = vec![0u32; usize::from(max_length) + 1];
        for &length in lengths {
            if length > 0 {
                bit_length_count[usize::from(length)] += 1;
            }
        }

        // RFC 1951 §3.2.2: the smallest code for each length, derived
        // from how many codes of the previous length were used.
        let mut next_code = vec![0u16; usize::from(max_length) + 1];
        let mut code: u32 = 0;
        for bits in 1..=usize::from(max_length) {
            code = (code + bit_length_count[bits - 1]) << 1;
            next_code[bits] = code as u16;
        }

        let mut codes = HashMap::new();
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let length_index = usize::from(length);
            let assigned_code = next_code[length_index];
            next_code[length_index] += 1;
            codes.insert((length, assigned_code), symbol as u16);
        }

        Self { codes, max_length }
    }

    /// Decodes one Huffman-coded symbol, reading one bit at a time and
    /// extending the candidate code most-significant-bit first (§3.2.2)
    /// until an assigned code of that length matches.
    pub(crate) fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, InflateError> {
        let mut code: u16 = 0;

        for length in 1..=self.max_length {
            code = (code << 1) | (reader.read_bits(1)? as u16);

            if let Some(&symbol) = self.codes.get(&(length, code)) {
                return Ok(symbol);
            }
        }

        Err(InflateError::InvalidHuffmanCode)
    }
}

/// The fixed literal/length Huffman code lengths (RFC 1951 §3.2.6):
/// symbols 0-143 (8 bits), 144-255 (9 bits), 256-279 (7 bits, including
/// the end-of-block symbol 256), 280-287 (8 bits; 286-287 are unused but
/// still occupy code space).
pub(crate) fn fixed_literal_length_code_lengths() -> [u8; 288] {
    let mut lengths = [0u8; 288];
    lengths[0..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..288].fill(8);
    lengths
}

/// The fixed distance Huffman code lengths (RFC 1951 §3.2.6): all 32
/// codes (0-29 valid, 30-31 unused but still occupy code space) are 5
/// bits.
pub(crate) fn fixed_distance_code_lengths() -> [u8; 32] {
    [5u8; 32]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test-only bit writer, packing bits least-significant-bit first
    /// per byte (matching `BitReader`), used to hand-encode small DEFLATE
    /// bit sequences worked out directly from the RFC 1951 examples.
    struct BitWriter {
        bytes: Vec<u8>,
        bit_pos: u8,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: vec![0],
                bit_pos: 0,
            }
        }

        fn write_bit(&mut self, bit: u8) {
            *self.bytes.last_mut().unwrap() |= bit << self.bit_pos;
            self.bit_pos += 1;
            if self.bit_pos == 8 {
                self.bit_pos = 0;
                self.bytes.push(0);
            }
        }

        /// Writes a Huffman code's bits most-significant-bit first, per
        /// RFC 1951 §3.2.2.
        fn write_huffman_code(&mut self, value: u16, length: u8) {
            for i in (0..length).rev() {
                self.write_bit(((value >> i) & 1) as u8);
            }
        }

        fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }

    #[test]
    fn bit_reader_reads_bits_least_significant_bit_first() {
        // 0b1011_0010 read LSB-first, 3 bits then 5 bits: 010, then 10110.
        let mut reader = BitReader::new(&[0b1011_0010]);

        assert_eq!(reader.read_bits(3).unwrap(), 0b010);
        assert_eq!(reader.read_bits(5).unwrap(), 0b10110);
    }

    #[test]
    fn bit_reader_reports_eof_past_the_end() {
        let mut reader = BitReader::new(&[0xFF]);
        reader.read_bits(8).unwrap();

        assert_eq!(reader.read_bits(1), Err(InflateError::UnexpectedEof));
    }

    #[test]
    fn huffman_table_reproduces_the_known_fixed_literal_codes() {
        // RFC 1951 §3.2.6 states these exact codes for the fixed
        // literal/length table; reproducing them from
        // `fixed_literal_length_code_lengths()` is a direct check that
        // the canonical-code construction (§3.2.2) is correct.
        let table = HuffmanTable::from_code_lengths(&fixed_literal_length_code_lengths());

        let mut writer = BitWriter::new();
        writer.write_huffman_code(0b0011_0000, 8); // symbol 0: first 8-bit code
        writer.write_huffman_code(0b0011_0000 + 65, 8); // symbol 65 ('A')
        writer.write_huffman_code(0b1011_1111, 8); // symbol 143: last 8-bit code in this group
        writer.write_huffman_code(0b0000_000, 7); // symbol 256: end-of-block
        writer.write_huffman_code(0b1100_0000, 8); // symbol 280: first code in the last group
        let bytes = writer.finish();

        let mut reader = BitReader::new(&bytes);
        assert_eq!(table.decode(&mut reader).unwrap(), 0);
        assert_eq!(table.decode(&mut reader).unwrap(), 65);
        assert_eq!(table.decode(&mut reader).unwrap(), 143);
        assert_eq!(table.decode(&mut reader).unwrap(), 256);
        assert_eq!(table.decode(&mut reader).unwrap(), 280);
    }

    #[test]
    fn huffman_table_reproduces_the_known_fixed_distance_codes() {
        let table = HuffmanTable::from_code_lengths(&fixed_distance_code_lengths());

        let mut writer = BitWriter::new();
        writer.write_huffman_code(0b00000, 5); // symbol 0
        writer.write_huffman_code(0b11111, 5); // symbol 31
        let bytes = writer.finish();

        let mut reader = BitReader::new(&bytes);
        assert_eq!(table.decode(&mut reader).unwrap(), 0);
        assert_eq!(table.decode(&mut reader).unwrap(), 31);
    }

    #[test]
    fn huffman_decode_reports_an_error_for_an_unassigned_code() {
        // Only two of the four possible 2-bit codes are assigned (to
        // symbols 1 and 2, since symbol 0 has length 0). A bit pattern
        // that decodes to the unassigned code 0b11 must be reported as
        // an error, not silently accepted.
        let table = HuffmanTable::from_code_lengths(&[0, 2, 2]);
        let mut reader = BitReader::new(&[0b0000_0011]);

        assert_eq!(
            table.decode(&mut reader),
            Err(InflateError::InvalidHuffmanCode)
        );
    }
}
