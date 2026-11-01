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

Pending.

## Checkpoint 3: pinned reader and final regression gates

Pending.
