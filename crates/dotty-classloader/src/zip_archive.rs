use crate::zip_reader::{ZipReadError, ZipReader};
use std::fmt;

const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4B50;
/// The fixed-size portion of the end-of-central-directory record (JAR/ZIP
/// APPNOTE.TXT §4.3.16), not counting the variable-length comment.
const END_OF_CENTRAL_DIRECTORY_RECORD_SIZE: usize = 22;
/// The comment length field is a `u16`, so a comment can be at most this
/// many bytes.
const MAX_COMMENT_LENGTH: usize = 0xFFFF;
const CENTRAL_DIRECTORY_FILE_HEADER_SIGNATURE: u32 = 0x0201_4B50;

/// Errors reading a ZIP (JAR) archive. Crate-private: callers only see
/// `JarClassPath`, which maps these into [`crate::class_path::ClassPathError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ZipError {
    /// No end-of-central-directory record was found. Either this isn't a
    /// ZIP archive, or it uses ZIP64 (which this crate does not support —
    /// see `docs/classloader.md`'s Milestone 2 scope).
    EndOfCentralDirectoryNotFound,
    /// A central directory file header didn't start with the expected
    /// signature, at the given byte offset.
    InvalidCentralDirectoryHeader {
        offset: usize,
    },
    /// A central directory entry's file name was not valid UTF-8, at the
    /// given byte offset.
    InvalidEntryName {
        offset: usize,
    },
    Read(ZipReadError),
}

impl fmt::Display for ZipError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EndOfCentralDirectoryNotFound => write!(
                formatter,
                "end-of-central-directory record not found (not a ZIP archive, or a ZIP64 archive, which is unsupported)"
            ),
            Self::InvalidCentralDirectoryHeader { offset } => write!(
                formatter,
                "invalid central directory file header at offset {offset}"
            ),
            Self::InvalidEntryName { offset } => {
                write!(
                    formatter,
                    "invalid (non-UTF-8) entry name at offset {offset}"
                )
            }
            Self::Read(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ZipError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::EndOfCentralDirectoryNotFound
            | Self::InvalidCentralDirectoryHeader { .. }
            | Self::InvalidEntryName { .. } => None,
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

/// One entry's metadata from a ZIP central directory (APPNOTE.TXT
/// §4.3.12): enough to find and extract that entry's bytes, without yet
/// reading them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CentralDirectoryEntry {
    pub(crate) name: String,
    pub(crate) compression_method: u16,
    pub(crate) crc32: u32,
    pub(crate) compressed_size: u32,
    pub(crate) uncompressed_size: u32,
    pub(crate) local_header_offset: u32,
}

/// Decodes `eocd.entry_count` central directory file headers starting at
/// `eocd.central_directory_offset`, bounded to `eocd.central_directory_size`
/// bytes.
pub(crate) fn decode_central_directory(
    bytes: &[u8],
    eocd: &EndOfCentralDirectory,
) -> Result<Vec<CentralDirectoryEntry>, ZipError> {
    let start = eocd.central_directory_offset as usize;
    let end = start + eocd.central_directory_size as usize;
    let mut reader = ZipReader::with_range(bytes, start, end)?;

    let mut entries = Vec::with_capacity(usize::from(eocd.entry_count));
    for _ in 0..eocd.entry_count {
        let header_offset = reader.position();

        let signature = reader.read_u32()?;
        if signature != CENTRAL_DIRECTORY_FILE_HEADER_SIGNATURE {
            return Err(ZipError::InvalidCentralDirectoryHeader {
                offset: header_offset,
            });
        }

        reader.read_u16()?; // version made by
        reader.read_u16()?; // version needed to extract
        reader.read_u16()?; // general purpose bit flag
        let compression_method = reader.read_u16()?;
        reader.read_u16()?; // last mod file time
        reader.read_u16()?; // last mod file date
        let crc32 = reader.read_u32()?;
        let compressed_size = reader.read_u32()?;
        let uncompressed_size = reader.read_u32()?;
        let file_name_length = reader.read_u16()?;
        let extra_field_length = reader.read_u16()?;
        let file_comment_length = reader.read_u16()?;
        reader.read_u16()?; // disk number start
        reader.read_u16()?; // internal file attributes
        reader.read_u32()?; // external file attributes
        let local_header_offset = reader.read_u32()?;

        let name_offset = reader.position();
        let name_bytes = reader.read_bytes(usize::from(file_name_length))?;
        let name =
            String::from_utf8(name_bytes.to_vec()).map_err(|_| ZipError::InvalidEntryName {
                offset: name_offset,
            })?;

        reader.read_bytes(usize::from(extra_field_length))?;
        reader.read_bytes(usize::from(file_comment_length))?;

        entries.push(CentralDirectoryEntry {
            name,
            compression_method,
            crc32,
            compressed_size,
            uncompressed_size,
            local_header_offset,
        });
    }

    Ok(entries)
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

    fn minimal_central_directory_header(
        name: &str,
        compression_method: u16,
        crc32: u32,
        compressed_size: u32,
        uncompressed_size: u32,
        local_header_offset: u32,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]); // signature
        bytes.extend_from_slice(&0u16.to_le_bytes()); // version made by
        bytes.extend_from_slice(&0u16.to_le_bytes()); // version needed to extract
        bytes.extend_from_slice(&0u16.to_le_bytes()); // general purpose bit flag
        bytes.extend_from_slice(&compression_method.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes()); // last mod file time
        bytes.extend_from_slice(&0u16.to_le_bytes()); // last mod file date
        bytes.extend_from_slice(&crc32.to_le_bytes());
        bytes.extend_from_slice(&compressed_size.to_le_bytes());
        bytes.extend_from_slice(&uncompressed_size.to_le_bytes());
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes()); // file name length
        bytes.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        bytes.extend_from_slice(&0u16.to_le_bytes()); // file comment length
        bytes.extend_from_slice(&0u16.to_le_bytes()); // disk number start
        bytes.extend_from_slice(&0u16.to_le_bytes()); // internal file attributes
        bytes.extend_from_slice(&0u32.to_le_bytes()); // external file attributes
        bytes.extend_from_slice(&local_header_offset.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes
    }

    #[test]
    fn decodes_two_central_directory_headers() {
        let mut central_directory = Vec::new();
        central_directory.extend(minimal_central_directory_header(
            "PoolSample.class",
            0,
            0x1234_5678,
            100,
            100,
            0,
        ));
        central_directory.extend(minimal_central_directory_header(
            "java/lang/Object.class",
            8,
            0x9ABC_DEF0,
            40,
            80,
            120,
        ));

        let eocd = EndOfCentralDirectory {
            entry_count: 2,
            central_directory_size: central_directory.len() as u32,
            central_directory_offset: 0,
        };

        let entries = decode_central_directory(&central_directory, &eocd).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "PoolSample.class");
        assert_eq!(entries[0].compression_method, 0);
        assert_eq!(entries[0].crc32, 0x1234_5678);
        assert_eq!(entries[0].compressed_size, 100);
        assert_eq!(entries[0].uncompressed_size, 100);
        assert_eq!(entries[0].local_header_offset, 0);

        assert_eq!(entries[1].name, "java/lang/Object.class");
        assert_eq!(entries[1].compression_method, 8);
        assert_eq!(entries[1].crc32, 0x9ABC_DEF0);
        assert_eq!(entries[1].compressed_size, 40);
        assert_eq!(entries[1].uncompressed_size, 80);
        assert_eq!(entries[1].local_header_offset, 120);
    }

    #[test]
    fn reports_an_invalid_header_when_the_signature_is_wrong() {
        let mut central_directory =
            minimal_central_directory_header("PoolSample.class", 0, 0, 0, 0, 0);
        central_directory[0] = 0x00; // corrupt the signature

        let eocd = EndOfCentralDirectory {
            entry_count: 1,
            central_directory_size: central_directory.len() as u32,
            central_directory_offset: 0,
        };

        assert_eq!(
            decode_central_directory(&central_directory, &eocd),
            Err(ZipError::InvalidCentralDirectoryHeader { offset: 0 })
        );
    }
}
