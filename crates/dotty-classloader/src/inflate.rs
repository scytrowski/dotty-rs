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
    /// A block header's `BTYPE` was `11` (reserved; RFC 1951 §3.2.3).
    InvalidBlockType,
    /// A stored block's `NLEN` was not the one's complement of `LEN`
    /// (§3.2.4).
    InvalidStoredBlockLength,
    /// A dynamic block's Huffman-table description (§3.2.7) was
    /// malformed: a repeat code appeared with nothing to repeat, a
    /// repeat ran past the expected number of code lengths, or the
    /// code-length alphabet decoded an unused symbol.
    InvalidDynamicHuffmanTable,
    /// A length/distance back-reference pointed at or before the start
    /// of the output produced so far.
    InvalidBackReference,
}

impl fmt::Display for InflateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(formatter, "unexpected end of DEFLATE stream"),
            Self::InvalidHuffmanCode => write!(formatter, "invalid Huffman code in DEFLATE stream"),
            Self::InvalidBlockType => write!(formatter, "reserved DEFLATE block type (BTYPE 11)"),
            Self::InvalidStoredBlockLength => write!(
                formatter,
                "stored block's NLEN is not the one's complement of LEN"
            ),
            Self::InvalidDynamicHuffmanTable => {
                write!(formatter, "malformed dynamic Huffman table description")
            }
            Self::InvalidBackReference => write!(
                formatter,
                "back-reference distance points before the start of the output"
            ),
        }
    }
}

impl std::error::Error for InflateError {}

/// A bit reader over a DEFLATE stream. Per RFC 1951 §3.1.1, every field
/// is packed least-significant-bit first within each byte; Huffman codes
/// are the one exception where the *value* built from consecutively read
/// bits is interpreted most-significant-bit first (§3.2.2) — that
/// distinction lives in [`HuffmanTable::decode`], not here.
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

    /// Discards any unread bits in the current byte, so the next read
    /// starts at a byte boundary (RFC 1951 §3.2.4, before a stored
    /// block's `LEN`/`NLEN`/data).
    pub(crate) fn align_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }

    /// Reads `count` raw bytes directly, without going through
    /// [`Self::read_bits`] (which is limited to 32 bits per call). Only
    /// meaningful once byte-aligned; callers call [`Self::align_to_byte`]
    /// first.
    pub(crate) fn read_aligned_bytes(&mut self, count: usize) -> Result<&'a [u8], InflateError> {
        let end = self
            .byte_pos
            .checked_add(count)
            .ok_or(InflateError::UnexpectedEof)?;
        if end > self.bytes.len() {
            return Err(InflateError::UnexpectedEof);
        }

        let bytes = &self.bytes[self.byte_pos..end];
        self.byte_pos = end;
        Ok(bytes)
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

/// Base lengths for length codes 257-285 (RFC 1951 §3.2.5, Table 3.2.5).
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
/// Extra bits to read after each length code, added to its base length.
const LENGTH_EXTRA_BITS: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Base distances for distance codes 0-29 (§3.2.5, Table 3.2.5).
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
/// Extra bits to read after each distance code, added to its base
/// distance.
const DISTANCE_EXTRA_BITS: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order in which a dynamic block's code-length-alphabet lengths are
/// transmitted (§3.2.7) — not the order the code-length symbols
/// themselves are numbered in.
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Decodes a dynamic block's header (§3.2.7): HLIT/HDIST/HCLEN counts,
/// the code-length alphabet's own Huffman table, then the literal/length
/// and distance code lengths it describes (with codes 16/17/18 as
/// run-length repeats), building both real Huffman tables.
fn decode_dynamic_tables(
    reader: &mut BitReader<'_>,
) -> Result<(HuffmanTable, HuffmanTable), InflateError> {
    let literal_length_count = reader.read_bits(5)? as usize + 257;
    let distance_count = reader.read_bits(5)? as usize + 1;
    let code_length_count = reader.read_bits(4)? as usize + 4;

    let mut code_length_lengths = [0u8; 19];
    for &position in CODE_LENGTH_ORDER.iter().take(code_length_count) {
        code_length_lengths[position] = reader.read_bits(3)? as u8;
    }
    let code_length_table = HuffmanTable::from_code_lengths(&code_length_lengths);

    let total = literal_length_count + distance_count;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        match code_length_table.decode(reader)? {
            symbol @ 0..=15 => lengths.push(symbol as u8),
            16 => {
                let repeat = 3 + reader.read_bits(2)?;
                let &previous = lengths
                    .last()
                    .ok_or(InflateError::InvalidDynamicHuffmanTable)?;
                lengths.extend(std::iter::repeat_n(previous, repeat as usize));
            }
            17 => {
                let repeat = 3 + reader.read_bits(3)?;
                lengths.extend(std::iter::repeat_n(0u8, repeat as usize));
            }
            18 => {
                let repeat = 11 + reader.read_bits(7)?;
                lengths.extend(std::iter::repeat_n(0u8, repeat as usize));
            }
            _ => return Err(InflateError::InvalidDynamicHuffmanTable),
        }
    }
    if lengths.len() != total {
        return Err(InflateError::InvalidDynamicHuffmanTable);
    }

    let literal_length_table = HuffmanTable::from_code_lengths(&lengths[..literal_length_count]);
    let distance_table = HuffmanTable::from_code_lengths(&lengths[literal_length_count..]);

    Ok((literal_length_table, distance_table))
}

/// Decodes one stored (uncompressed) block (§3.2.4): byte-aligns, reads
/// `LEN`/`NLEN` (verifying they're complementary), then copies `LEN`
/// bytes directly into `output`.
fn decode_stored_block(
    reader: &mut BitReader<'_>,
    output: &mut Vec<u8>,
) -> Result<(), InflateError> {
    reader.align_to_byte();

    let length = reader.read_bits(16)? as u16;
    let length_complement = reader.read_bits(16)? as u16;
    if length_complement != !length {
        return Err(InflateError::InvalidStoredBlockLength);
    }

    output.extend_from_slice(reader.read_aligned_bytes(usize::from(length))?);
    Ok(())
}

/// Decodes one Huffman-coded block (fixed or dynamic; §3.2.5): literal
/// bytes are appended directly, length/distance pairs copy already-
/// produced output (LZ77 back-references), and the end-of-block symbol
/// (256) ends the block.
fn decode_huffman_block(
    reader: &mut BitReader<'_>,
    literal_length_table: &HuffmanTable,
    distance_table: &HuffmanTable,
    output: &mut Vec<u8>,
) -> Result<(), InflateError> {
    loop {
        match literal_length_table.decode(reader)? {
            symbol @ 0..=255 => output.push(symbol as u8),
            256 => return Ok(()),
            symbol @ 257..=285 => {
                let index = usize::from(symbol - 257);
                let length =
                    u32::from(LENGTH_BASE[index]) + reader.read_bits(LENGTH_EXTRA_BITS[index])?;

                let distance_symbol = distance_table.decode(reader)?;
                let distance_index = usize::from(distance_symbol);
                let Some(&distance_base) = DISTANCE_BASE.get(distance_index) else {
                    return Err(InflateError::InvalidBackReference);
                };
                let distance = usize::try_from(
                    u32::from(distance_base)
                        + reader.read_bits(DISTANCE_EXTRA_BITS[distance_index])?,
                )
                .expect("distance always fits in usize on supported targets");

                if distance == 0 || distance > output.len() {
                    return Err(InflateError::InvalidBackReference);
                }

                let start = output.len() - distance;
                for i in 0..length as usize {
                    output.push(output[start + i]);
                }
            }
            _ => return Err(InflateError::InvalidHuffmanCode), // 286/287: unused
        }
    }
}

/// Decompresses a raw DEFLATE (RFC 1951) stream — the format ZIP's
/// compression method 8 uses, with no zlib or gzip framing around it.
pub(crate) fn inflate(compressed: &[u8]) -> Result<Vec<u8>, InflateError> {
    let mut reader = BitReader::new(compressed);
    let mut output = Vec::new();

    let fixed_literal_length_table =
        HuffmanTable::from_code_lengths(&fixed_literal_length_code_lengths());
    let fixed_distance_table = HuffmanTable::from_code_lengths(&fixed_distance_code_lengths());

    loop {
        let is_final_block = reader.read_bits(1)? == 1;
        let block_type = reader.read_bits(2)?;

        match block_type {
            0 => decode_stored_block(&mut reader, &mut output)?,
            1 => decode_huffman_block(
                &mut reader,
                &fixed_literal_length_table,
                &fixed_distance_table,
                &mut output,
            )?,
            2 => {
                let (literal_length_table, distance_table) = decode_dynamic_tables(&mut reader)?;
                decode_huffman_block(
                    &mut reader,
                    &literal_length_table,
                    &distance_table,
                    &mut output,
                )?;
            }
            _ => return Err(InflateError::InvalidBlockType),
        }

        if is_final_block {
            break;
        }
    }

    Ok(output)
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

        /// Writes a non-Huffman field's bits least-significant-bit
        /// first, matching `BitReader::read_bits`.
        fn write_bits(&mut self, value: u32, count: u8) {
            for i in 0..count {
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
        writer.write_huffman_code(0b000_0000, 7); // symbol 256: end-of-block
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

    #[test]
    fn inflate_decodes_a_fixed_huffman_block_with_a_back_reference() {
        let mut writer = BitWriter::new();
        writer.write_bits(1, 1); // BFINAL = 1
        writer.write_bits(0b01, 2); // BTYPE = 01 (fixed Huffman)
        writer.write_huffman_code(0x30 + u16::from(b'A'), 8); // literal 'A'
        writer.write_huffman_code(0x30 + u16::from(b'B'), 8); // literal 'B'
        writer.write_huffman_code(2, 7); // length code 258 -> length 4
        writer.write_huffman_code(1, 5); // distance code 1 -> distance 2
        writer.write_huffman_code(0, 7); // end-of-block (symbol 256)
        let bytes = writer.finish();

        // "AB" + a length-4/distance-2 back-reference, copied byte by
        // byte (the copy source overlaps the destination, since
        // distance < length): A,B,A,B,A,B -> "ABABAB".
        assert_eq!(inflate(&bytes).unwrap(), b"ABABAB");
    }

    fn extract_raw_deflate_bytes(jar_bytes: &[u8], entry_name: &str) -> Vec<u8> {
        use crate::zip_archive::{EndOfCentralDirectory, decode_central_directory};
        use crate::zip_reader::ZipReader;

        let eocd = EndOfCentralDirectory::locate_and_decode(jar_bytes).unwrap();
        let entries = decode_central_directory(jar_bytes, &eocd).unwrap();
        let entry = entries
            .iter()
            .find(|entry| entry.name == entry_name)
            .expect("entry should be present in the central directory");
        assert_eq!(
            entry.compression_method, 8,
            "fixture entry should be DEFLATE-compressed"
        );

        // Mirrors ZipArchive::extract_entry's local-header-skipping
        // logic, stopping short of dispatching on compression method
        // (ZipArchive doesn't support method 8 until the next increment
        // wires inflate() into it).
        let mut reader = ZipReader::with_range(
            jar_bytes,
            entry.local_header_offset as usize,
            jar_bytes.len(),
        )
        .unwrap();
        reader.read_u32().unwrap(); // local file header signature
        reader.read_u16().unwrap(); // version needed to extract
        reader.read_u16().unwrap(); // general purpose bit flag
        reader.read_u16().unwrap(); // compression method
        reader.read_u16().unwrap(); // last mod file time
        reader.read_u16().unwrap(); // last mod file date
        reader.read_u32().unwrap(); // crc-32
        reader.read_u32().unwrap(); // compressed size
        reader.read_u32().unwrap(); // uncompressed size
        let file_name_length = reader.read_u16().unwrap();
        let extra_field_length = reader.read_u16().unwrap();
        reader.read_bytes(usize::from(file_name_length)).unwrap();
        reader.read_bytes(usize::from(extra_field_length)).unwrap();

        reader
            .read_bytes(entry.compressed_size as usize)
            .unwrap()
            .to_vec()
    }

    #[test]
    fn inflate_decompresses_a_real_deflate_compressed_jar_entry() {
        let jar_bytes = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/pool_sample_jar/pool_sample_deflate.jar"),
        )
        .expect("pool_sample_deflate.jar fixture should exist");
        let compressed = extract_raw_deflate_bytes(&jar_bytes, "PoolSample.class");

        let decompressed = inflate(&compressed).unwrap();

        let expected = std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../dotty-classfile/tests/fixtures/pool_sample/PoolSample.class"),
        )
        .unwrap();
        assert_eq!(decompressed, expected);
        assert_eq!(crate::crc32::checksum(&decompressed), 0xaa43_8c97);
    }
}
