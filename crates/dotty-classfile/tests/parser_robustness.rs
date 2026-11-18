//! Checks EOF diagnostics for truncated class files and exercises decoding
//! across deterministic byte mutations.

use dotty_classfile::classfile::{
    AttributeError, ClassFile, ClassFileError, ClassFileHeaderError, ConstantPoolError, ReadError,
    Reader,
};

const POOL_SAMPLE_CLASS: &[u8] = include_bytes!("fixtures/pool_sample/PoolSample.class");
const NESTED_SAMPLE: &[u8] = include_bytes!("fixtures/nested_sample/NestedSample.class");
const NESTED_SAMPLE_INNER: &[u8] =
    include_bytes!("fixtures/nested_sample/NestedSample$Inner.class");
const NESTED_SAMPLE_LOCAL_RUNNABLE: &[u8] =
    include_bytes!("fixtures/nested_sample/NestedSample$1LocalRunnable.class");
const SHAPE: &[u8] = include_bytes!("fixtures/sealed_record_sample/Shape.class");
const SHAPE_CIRCLE: &[u8] = include_bytes!("fixtures/sealed_record_sample/Shape$Circle.class");

const FIXTURES: &[&[u8]] = &[
    POOL_SAMPLE_CLASS,
    NESTED_SAMPLE,
    NESTED_SAMPLE_INNER,
    NESTED_SAMPLE_LOCAL_RUNNABLE,
    SHAPE,
    SHAPE_CIRCLE,
];

fn decode(bytes: &[u8]) {
    let mut reader = Reader::new(bytes);
    let _ = ClassFile::decode(&mut reader);
}

#[test]
fn every_truncated_fixture_prefix_reports_the_missing_bytes() {
    for fixture in FIXTURES {
        let mut full = Reader::new(fixture);
        ClassFile::decode(&mut full).unwrap();
        assert!(full.is_at_end());

        for length in 0..fixture.len() {
            let error = ClassFile::decode(&mut Reader::new(&fixture[..length])).unwrap_err();
            let read_error = match error {
                ClassFileError::Read(error)
                | ClassFileError::Header(ClassFileHeaderError::Read(error))
                | ClassFileError::ConstantPool(ConstantPoolError::Read(error))
                | ClassFileError::Attribute(AttributeError::Read(error)) => error,
                other => panic!("expected EOF for prefix of {length} bytes, got {other:?}"),
            };
            let ReadError::UnexpectedEof {
                offset,
                needed,
                remaining,
            } = read_error
            else {
                panic!("expected EOF for prefix of {length} bytes, got {read_error:?}");
            };
            assert_eq!(offset + remaining, length);
            assert!(needed > remaining);
        }
    }
}

#[test]
fn handles_deterministic_corruptions_of_a_valid_fixture_without_panicking() {
    for fixture in FIXTURES {
        for offset in 0..fixture.len() {
            let mut corrupted = fixture.to_vec();
            corrupted[offset] ^= 0xFF;
            decode(&corrupted);
        }
    }
}

#[test]
fn handles_random_bytes_without_panicking() {
    // A small deterministic xorshift PRNG, so this test needs no RNG
    // dependency and is reproducible across runs.
    let mut state: u64 = 0x1234_5678_9abc_def0;
    for fixture in FIXTURES {
        let mut bytes = vec![0u8; fixture.len()];
        for byte in bytes.iter_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = (state & 0xFF) as u8;
        }
        decode(&bytes);
    }
}
