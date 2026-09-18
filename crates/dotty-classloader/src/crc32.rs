use std::sync::OnceLock;

/// The standard IEEE 802.3 CRC-32 polynomial in reversed (LSB-first) form,
/// as used by ZIP/PNG/gzip and every other common CRC-32 variant.
const POLYNOMIAL: u32 = 0xEDB8_8320;

fn table() -> &'static [u32; 256] {
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();

    TABLE.get_or_init(|| {
        let mut table = [0u32; 256];

        for (index, entry) in table.iter_mut().enumerate() {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 != 0 {
                    POLYNOMIAL ^ (value >> 1)
                } else {
                    value >> 1
                };
            }
            *entry = value;
        }

        table
    })
}

/// Computes the IEEE 802.3 CRC-32 checksum of `bytes`, as used by the ZIP
/// file format to detect corrupt entries.
pub fn checksum(bytes: &[u8]) -> u32 {
    let table = table();
    let mut crc = 0xFFFF_FFFFu32;

    for &byte in bytes {
        let index = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = table[index] ^ (crc >> 8);
    }

    !crc
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
