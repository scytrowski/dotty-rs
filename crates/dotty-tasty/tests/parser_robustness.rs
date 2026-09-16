use std::panic::{AssertUnwindSafe, catch_unwind};

use dotty_tasty::tasty::TastyFile;

const FIXTURE: &[u8] = include_bytes!("fixtures/case_class/Point.tasty");

fn assert_parser_does_not_panic(input: &[u8], label: &str) {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = TastyFile::parse(input);
        let _ = TastyFile::parse_scala_3_9(input);
        let _ = TastyFile::parse_and_validate_scala_3_9_with_max_ast_index_depth(input, 32);
    }));

    assert!(result.is_ok(), "parser panicked for {label}");
}

#[test]
fn rejects_every_truncated_fixture_prefix_without_panicking() {
    for end in 0..FIXTURE.len() {
        assert_parser_does_not_panic(&FIXTURE[..end], &format!("fixture prefix of {end} bytes"));
    }
}

#[test]
fn handles_deterministic_corruptions_of_a_valid_fixture_without_panicking() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;

    for case in 0..256 {
        let mut input = FIXTURE.to_vec();
        let mutation_count = (next_random(&mut state) % 16 + 1) as usize;

        for _ in 0..mutation_count {
            let index = (next_random(&mut state) as usize) % input.len();
            input[index] ^= (next_random(&mut state) as u8).max(1);
        }

        assert_parser_does_not_panic(&input, &format!("corrupted fixture case {case}"));
    }
}

#[test]
fn handles_random_bytes_without_panicking() {
    let mut state = 0x243f_6a88_85a3_08d3_u64;

    for case in 0..512 {
        let length = (next_random(&mut state) % 2048) as usize;
        let mut input = Vec::with_capacity(length);
        for _ in 0..length {
            input.push(next_random(&mut state) as u8);
        }

        assert_parser_does_not_panic(&input, &format!("random input case {case}"));
    }
}

fn next_random(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1);
    *state
}
