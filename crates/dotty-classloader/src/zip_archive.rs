use crate::zip_reader::{ZipReadError, ZipReader};
use std::fmt;

const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4B50;
/// The fixed-size portion of the end-of-central-directory record (JAR/ZIP
/// APPNOTE.TXT §4.3.16), not counting the variable-length comment.
const END_OF_CENTRAL_DIRECTORY_RECORD_SIZE: usize = 22;
/// The comment length field is a `u16`, so a comment can be at most this
/// many bytes.
const MAX_COMMENT_LENGTH: usize = 0xFFFF;

/// Errors reading a ZIP (JAR) archive. Crate-private: callers only see
/// `JarClassPath`, which maps these into [`crate::class_path::ClassPathError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ZipError {
    /// No end-of-central-directory record was found. Either this isn't a
    /// ZIP archive, or it uses ZIP64 (which this crate does not support —
    /// see `docs/classloader.md`'s Milestone 2 scope).
    EndOfCentralDirectoryNotFound,
    Read(ZipReadError),
}

impl fmt::Display for ZipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndOfCentralDirectoryNotFound => write!(
                formatter,
                "end-of-central-directory record not found (not a ZIP archive, or a ZIP64 archive, which is unsupported)"
            ),
            Self::Read(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ZipError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EndOfCentralDirectoryNotFound => None,
            Self::Read(error) => Some(error),
        }
    }
}

impl From<ZipReadError> for ZipError {
    fn from(error: ZipReadError) -> Self {
        Self::Read(error)
    }
}

/// The fixed fields of a ZIP end-of-central-directory record that this
/// crate needs (APPNOTE.TXT §4.3.16). Multi-disk archives and ZIP64 are
/// not supported: this always reads the single-disk fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EndOfCentralDirectory {
    pub(crate) entry_count: u16,
    pub(crate) central_directory_size: u32,
    pub(crate) central_directory_offset: u32,
}

impl EndOfCentralDirectory {
    /// Locates and decodes the end-of-central-directory record by
    /// scanning backward from the end of `bytes`. The record sits at the
    /// very end of a ZIP file, after an optional comment of up to 65535
    /// bytes, so the search window is bounded to the trailing
    /// `22 + 65535` bytes. A candidate is only accepted once its own
    /// comment-length field is checked against how many bytes actually
    /// follow it — this rules out a signature that happens to appear
    /// inside the comment itself, to the left of the real record.
    pub(crate) fn locate_and_decode(bytes: &[u8]) -> Result<Self, ZipError> {
        let len = bytes.len();
        if len < END_OF_CENTRAL_DIRECTORY_RECORD_SIZE {
            return Err(ZipError::EndOfCentralDirectoryNotFound);
        }

        let search_floor =
            len.saturating_sub(END_OF_CENTRAL_DIRECTORY_RECORD_SIZE + MAX_COMMENT_LENGTH);
        let search_ceiling = len - END_OF_CENTRAL_DIRECTORY_RECORD_SIZE;

        for offset in (search_floor..=search_ceiling).rev() {
            let Ok(mut reader) = ZipReader::with_range(bytes, offset, len) else {
                continue;
            };

            let Ok(signature) = reader.read_u32() else {
                continue;
            };
            if signature != END_OF_CENTRAL_DIRECTORY_SIGNATURE {
                continue;
            }

            // These reads cannot fail: the loop bounds guarantee at least
            // `END_OF_CENTRAL_DIRECTORY_RECORD_SIZE` bytes remain from
            // `offset`, and only 18 more bytes are read here.
            reader.read_u16()?; // number of this disk
            reader.read_u16()?; // disk with the start of the central directory
            reader.read_u16()?; // entries in the central directory on this disk
            let entry_count = reader.read_u16()?;
            let central_directory_size = reader.read_u32()?;
            let central_directory_offset = reader.read_u32()?;
            let comment_length = reader.read_u16()?;

            let expected_len =
                offset + END_OF_CENTRAL_DIRECTORY_RECORD_SIZE + usize::from(comment_length);
            if expected_len == len {
                return Ok(Self {
                    entry_count,
                    central_directory_size,
                    central_directory_offset,
                });
            }
        }

        Err(ZipError::EndOfCentralDirectoryNotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_eocd(entry_count: u16, cd_size: u32, cd_offset: u32, comment: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]); // signature
        bytes.extend_from_slice(&0u16.to_le_bytes()); // disk number
        bytes.extend_from_slice(&0u16.to_le_bytes()); // disk with start of cd
        bytes.extend_from_slice(&entry_count.to_le_bytes()); // entries on this disk
        bytes.extend_from_slice(&entry_count.to_le_bytes()); // total entries
        bytes.extend_from_slice(&cd_size.to_le_bytes());
        bytes.extend_from_slice(&cd_offset.to_le_bytes());
        bytes.extend_from_slice(&(comment.len() as u16).to_le_bytes());
        bytes.extend_from_slice(comment);
        bytes
    }

    #[test]
    fn locates_the_eocd_with_no_comment() {
        let bytes = minimal_eocd(3, 100, 50, &[]);

        let eocd = EndOfCentralDirectory::locate_and_decode(&bytes).unwrap();

        assert_eq!(eocd.entry_count, 3);
        assert_eq!(eocd.central_directory_size, 100);
        assert_eq!(eocd.central_directory_offset, 50);
    }

    #[test]
    fn locates_the_eocd_with_a_comment() {
        let bytes = minimal_eocd(1, 40, 10, b"hello, jar");

        let eocd = EndOfCentralDirectory::locate_and_decode(&bytes).unwrap();

        assert_eq!(eocd.entry_count, 1);
        assert_eq!(eocd.central_directory_size, 40);
        assert_eq!(eocd.central_directory_offset, 10);
    }

    #[test]
    fn locates_the_eocd_when_preceded_by_unrelated_archive_bytes() {
        let mut bytes = vec![0xAB; 128]; // stand-in for local headers + central directory
        bytes.extend(minimal_eocd(2, 64, 20, &[]));

        let eocd = EndOfCentralDirectory::locate_and_decode(&bytes).unwrap();

        assert_eq!(eocd.entry_count, 2);
    }

    #[test]
    fn reports_not_found_when_the_signature_is_missing() {
        let bytes = vec![0u8; 22];

        assert_eq!(
            EndOfCentralDirectory::locate_and_decode(&bytes),
            Err(ZipError::EndOfCentralDirectoryNotFound)
        );
    }

    #[test]
    fn reports_not_found_for_a_too_short_buffer() {
        let bytes = vec![0x50, 0x4B, 0x05, 0x06];

        assert_eq!(
            EndOfCentralDirectory::locate_and_decode(&bytes),
            Err(ZipError::EndOfCentralDirectoryNotFound)
        );
    }
}
