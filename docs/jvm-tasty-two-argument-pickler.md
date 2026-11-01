# Two-argument sibling constructor calls in Rust-produced TASTy

This records implementation evidence for [issue #1916](https://github.com/scytrowski/dotty-rs/issues/1916). The accepted source is the unchanged three-class `PairFactory.scala` used by the pinned Scala 3.9.0 oracle for [issue #1913](jvm-tasty-multi-argument-constructor-calls.md). Source typing and JVM emission are already covered by the earlier issue; this increment is limited to TASTy writing and decoding.

## Checkpoint 1: characterize the writer boundary

The canonical writer input uses one lexer/parser/namer/typer/JVM planning session and the ordered immutable two-argument constructor receipt added in #1914 and validated in #1915. The existing focused test `rust_batch_writer_baseline_rejects_the_validated_two_argument_pair_at_expression_emission` confirms the first failure before this change:

```text
PairedBatchArtifactError::ClassArtifact { class_index: 2, .. }
  -> PairedArtifactError::TastyPickle
  -> PickleError::Unsupported { feature: BodyExpression, .. }
```

The failure is the explicit constructor-application `args.len() != 1` guard in `expression.rs`. The full batch API returns no partial artifacts. This happens after paired planning and the ordered source/typed proof succeed, so it is a writer limitation rather than a source or proof failure. The constructor `SELECTin` projection separately accepts only one formal, and class-state output registers only a singleton nested-call receipt; both need a bounded paired path.

The pinned oracle characterization in [the #1913 evidence](jvm-tasty-multi-argument-constructor-calls.md#pinned-source-and-oracle) establishes the supported signature and shape: outer `Pair.<init>(Counter, Counter): Pair`, with two left-to-right sibling `Counter.<init>(Int): Counter` applications. The genuine original argument leaves target `PairFactory.create.first` and `.second` by their respective direct formal addresses. The encoding must preserve the original distinct Apply roots, selected class/formal types, and source ordering; it must not infer arbitrary multi-argument calls.

Before writer changes, checked-in pre-refactor semantic pickler byte fixtures have these SHA-256 values:

| Fixture | SHA-256 |
| --- | --- |
| `Answer.hex` | `5076d8093d7c874bdb7e0f3b54a8923f14a8e9f248c3b7713d5a10b2f48a43db` |
| `Operations.hex` | `d91db1fa9689fef497d79d35311483060e704ef7f2460c9f2bf355d977e84f34` |
| `Parameters.hex` | `7a995a93ada3040cbd9192fd4b04c06ced4831e990755ebc5bc35bab8364d801` |

The three Scala-generated oracle sidecars are also unchanged baseline evidence, not Rust output: `Counter.tasty` SHA-256 `f3c1adc42db4979c529c3566d88652c24fed026dd0d8550295592e16d3945a89`, `Pair.tasty` SHA-256 `df8217316bd32e21b15eccf27b5e2019c736e49e514bb544f465ef7227f70c8b`, and `PairFactory.tasty` SHA-256 `2232b74c794b64107dc96b9535230378721f0d3274709fcc47437af89e2f1077`.

The baseline test ran with pinned Rust 1.98.1 using worktree-local `TMPDIR` and `CARGO_TARGET_DIR`, with `CARGO_BUILD_JOBS=2`:

```text
cargo test -p dotty --test jvm_tasty_multi_argument_constructor_calls \
  rust_batch_writer_baseline --locked -- --nocapture
1 passed
```

## Checkpoint 2: bounded writer

The validated class-state output path now registers the classes from both immutable nested-call receipts, skipping duplicate class identities. Constructor selection accepts the existing singleton `(Int)Class` and `(PlannedSiblingRef)Class` forms, or the new ordered pair whose two reference formals name the same planned sibling class. `SignedMany` keeps both formal type references in source order. Expression lowering accepts only one or two arguments for the selected constructor path and visits each original typed argument in order with the same traversal state and budget.

The Rust producer regression calls `pickle()` for all three class plans inside one `with_paired_jvm_tasty_batch_plan` callback. Fresh-session reruns compare all sidecar bytes. The decoder asserts the Factory method's two `Int` PARAMs and `Pair` return, three distinct APPLY addresses, the two-reference Pair constructor signature, both nested one-Int Counter signatures, and direct leaves to the original first and second formal addresses. Renamed package/class and reordered declaration controls are also byte deterministic.

Invalid second receipts are exercised through `pickle()` after body validation; no output is returned for missing, duplicated/shared, reversed, wrong-class, wrong-constructor/owner, wrong-result, stale-leaf, or wrong-source-edge receipts. Source controls reject missing and third outer constructor arguments at source typing with `ApplicationArityMismatch`; deeper nested calls and branch arguments fail before producer callbacks because the ordered proof requires the original factory-formal leaf. Existing one-argument, nested New, factory-result, local-value, Pair definition, and primitive pickler regressions pass unchanged, including the frozen pre-refactor primitive byte fixtures.

The public paired class/TASTy batch helper has a separate downstream contract limitation: after the TASTy writer succeeds, its cross-class invocation validation rejects the two-argument Pair constructor because that validator currently recognizes only direct zero-argument cross-class calls. That validator/JVM integration is outside this issue's approved writer scope. The issue's acceptance gate therefore consumes the original writer-produced `.tasty` sidecars directly from the same validated source session; it makes no paired Rust classfile/runtime claim.

## Checkpoint 3: pinned reader and regression gates

The strict regression, enabled with `DOTTY_REQUIRE_SCALA_TASTY_MULTI_ARGUMENT_CONSUMER=1`, writes the three direct Rust TASTy outputs into an empty task-local directory. Pinned Scala 3.9.0 `-decompile` accepts Counter, Pair, and PairFactory; decompiling PairFactory recovers its exact two-argument `create` signature and ordered `new Pair(new Counter(first), new Counter(second))` body. Pinned `-from-tasty` consumes all three Rust sidecars and reconstructs the three classes. This checks Scala TASTy reader interoperability only; it does not use or assert Rust JVM classfiles.

The Scala source oracle test remains a separate control: it recompiles the checked-in source at revision `777528f19a58e794c9954a42f433373472ec57f8`, confirms all six compiler-generated class/TASTy artifacts byte for byte, and runs its independent Scala client. The Rust writer test compares fresh-session outputs and source variants independently of those Scala-generated fixtures.

Validation passed with Rust 1.98.1, Temurin 21.0.13, Scala 3.9.0 `777528f19a58e794c9954a42f433373472ec57f8`, `CARGO_TARGET_DIR` and `TMPDIR` inside the issue worktree, and at most two Cargo build jobs:

```text
cargo test -p dotty-tasty-pickler --all-targets --locked
cargo test -p dotty --test jvm_tasty_multi_argument_constructor_calls \
  --test jvm_tasty_multi_argument_constructor_matrix --locked
DOTTY_REQUIRE_SCALA_TASTY_MULTI_ARGUMENT_CONSUMER=1 \
  cargo test -p dotty --test jvm_tasty_multi_argument_constructor_calls \
  strict_pinned_scala_consumer --locked -- --nocapture
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
cargo clippy --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo test --workspace --all-targets --locked
DOTTY_CLASSFILE_REQUIRE_JDK=1 cargo test \
  -p dotty-classfile --test classfile_jvm_encoder \
  -p dotty-jvm --lib \
  -p dotty --test jvm_source_compilation \
  --test jvm_stateful_class_contract --locked -- --nocapture
git diff --check
```

The standard workspace suite passed after the singleton-reference compatibility correction. The required JDK gate passed with `JAVA_HOME` and PATH pointing to Temurin 21.0.13 and a relative worktree-local JVM temp directory. The strict Scala test separately asserted the pinned compiler revision and JDK version.
