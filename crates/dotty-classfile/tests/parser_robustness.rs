//! Confirms that version/constant-pool decoding never panics, whatever
//! bytes it is given — malformed input must produce a typed error, not a
//! crash (`AGENTS.md`: "Library code must not panic on malformed TASTy
//! input"; the same principle applies here).

use dotty_classfile::class_file::ClassFileVersion;
use dotty_classfile::constant_pool::ConstantPool;
use dotty_classfile::reader::Reader;

const POOL_SAMPLE_CLASS: &[u8] = include_bytes!("fixtures/pool_sample/PoolSample.class");

fn decode(bytes: &[u8]) {
    let mut reader = Reader::new(bytes);
    if ClassFileVersion::decode(&mut reader).is_ok() {
        let _ = ConstantPool::decode(&mut reader);
    }
}

#[test]
fn rejects_every_truncated_fixture_prefix_without_panicking() {
    for length in 0..=POOL_SAMPLE_CLASS.len() {
        decode(&POOL_SAMPLE_CLASS[..length]);
    }
}

#[test]
fn handles_deterministic_corruptions_of_a_valid_fixture_without_panicking() {
    for offset in 0..POOL_SAMPLE_CLASS.len() {
        let mut corrupted = POOL_SAMPLE_CLASS.to_vec();
        corrupted[offset] ^= 0xFF;
        decode(&corrupted);
    }
}

#[test]
fn handles_random_bytes_without_panicking() {
    // A small deterministic xorshift PRNG, so this test needs no RNG
    // dependency and is reproducible across runs.
    let mut state: u64 = 0x1234_5678_9abc_def0;
    let mut bytes = vec![0u8; POOL_SAMPLE_CLASS.len()];
    for byte in bytes.iter_mut() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state & 0xFF) as u8;
    }

    decode(&bytes);
}
