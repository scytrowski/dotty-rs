# Six-root lexical closure Typer gate

This gate implements issue #1889's frozen six-positive acceptance matrix. It
types each exact source `DefDef` through Lexer → Parser → Namer →
`SourceTyper::type_method_definition` twice. Each invocation builds fresh
stores, package state, semantic indexes, source trees, typed arenas, and Typer
instances. The test compares normalized owner, method, result, capture, source
mapping, and span receipts; it never compares arena-local numeric IDs.

## Frozen roots and receipt

The baseline is `9dc0fef63dd4245c0b23a4fb438eb19bbb2fd343` (current main when
the gate was added, including merged #1888 / PR #1893). Source hashes are
recorded here so fixture edits cannot silently change the six-case denominator:

| Source root | SHA-256 |
| --- | --- |
| strict class `C` with `def identity: Int => Int = x => x` | `a74075e2e4aef1d41ffb33b1ee840dfd58405f6f8a0b3e3886b90fa61029cc41` |
| `lexical-closure-contract/MethodParameters.scala` | `a2103c926f2d280b21d178e438a2735a214f9039cf62e1150f1cb5f0a3984744` |
| `lexical-closure-contract/LocalValues.scala` | `4c6c73e81efdbdaa72f13437a4fa5000a0c7fa975ea8382640f508791e8fe65e` |
| `lexical-closure-contract/ThisCapture.scala` | `2d5c29fc188904cbbd7409784cdb935bccc9f79d576e4d4099497e320ac259e2` |
| `lexical-closure-contract/SameOwnerMembers.scala` | `3ec081068d583a0aebfbc2faad83c048deb44bd614f6c942b112e71814743375` |
| `lexical-closure-contract/CombinedCaptures.scala` | `862a0a0ef16adccdfd1227ef23562bce22428f43ef0be9d824ae6796a04c1dca` |
| combined Scala 3.9.0 normalized typer snapshot | `7a06140879ad2f43ad34b800f242cce4525cd3169305697bed2ac10bafe5b316` |

The exact method identities, owners, result shapes, ordered capture identities,
and both normalized session receipts are checked in
[`six-root-gate.tsv`](../crates/dotty-typer/tests/fixtures/lexical-closure-contract/six-root-gate.tsv).
The first integrated run accepted all six roots in both fresh sessions. The
capture counts are `0, 1, 1, 1, 1, 2`; the combined row is exactly
`[Calculator.this, delta]`.

For each row the test verifies the original source method symbol, complete
signature and owned typed `DefDef`, full RHS result conformance, typed closure,
synthetic lambda method/body/parameter, capture type and canonical owner,
source-to-typed method/RHS/body/identifier/call mappings, and preserved source
spans. The local-value row also checks that the initialized `bias` `ValDef`
remains in the typed block. The lambda parameter is explicitly excluded from
every capture environment. No production semantics or public API changed.

## Validation

Run the integrated six-root gate:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 \
CARGO_TARGET_DIR=target/issue-1889-six-root \
cargo test -p dotty-typer --lib \
  six_frozen_roots_type_as_complete_methods_in_two_independent_sessions --locked
```

Run the surrounding focused regressions and repository checks:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_TARGET_DIR=target/issue-1889-six-root \
cargo test -p dotty-typer --lib --locked
cargo fmt --all -- --check
python3 tools/check-frontend-boundaries.py
CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_TARGET_DIR=target/issue-1889-six-root \
cargo clippy -p dotty-typer --all-targets --locked -- -D warnings
git diff --check
```

The combined Scala 3.9.0 source/snapshot and its wrong-argument and wrong-result
controls were captured and verified by the pinned oracle in PR #1893. This
issue consumes that exact fixture and does not regenerate the oracle.
