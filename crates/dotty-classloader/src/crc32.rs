/// Computes the IEEE 802.3 CRC-32 checksum of `bytes`, as used by the ZIP
/// file format to detect corrupt entries. `crc32fast` uses hardware-accelerated
/// implementations when the target CPU supports them.
pub fn checksum(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_of_empty_input_is_zero() {
        assert_eq!(checksum(&[]), 0x0000_0000);
    }

    #[test]
    fn checksum_of_the_standard_ascii_test_vector() {
        assert_eq!(checksum(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn checksum_differs_for_different_input() {
        assert_ne!(checksum(b"abc"), checksum(b"abd"));
    }
}
