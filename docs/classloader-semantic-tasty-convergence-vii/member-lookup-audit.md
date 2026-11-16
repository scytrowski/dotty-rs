# Authentic `InfoWrapped.m` selected term lookup

## Scope and frozen inputs

This is the A1 evidence-only audit for issue #2329. The baseline is `main`
commit `664f2515746867e37610ec2255172936fd5f7b97`; the focused lookup test was
first recorded in test commit `2e60efa3305ed17ca07688ce2825dc6bcaeb61e3` and
its normalized fresh-store comparison is at test commit
`58cd7780a79c449bfecdff06023a1c93113371c6`.
No production resolver, loader, core-resolution or unpickler behavior is
changed. The pinned source reference remains Scala 3.9.0 revision
`777528f19a58e794c9954a42f433373472ec57f8`, with Rust 1.98.1 and Temurin
JDK `21.0.13+9-LTS`.

| Original fixture | SHA-256 |
| --- | --- |
| `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoBase.tasty` | `e4d58a733e8cc9a516b5d248dd6fb97acb2cb19d763e1e390cfcd1249bd7f5f9` |
| `crates/dotty-tasty-unpickler/tests/fixtures/semantic/InfoWrapped.tasty` | `07cca416641dda27a333df99603bf7b119d0bc5660098c5d378a09064e1630fd` |

The original `InfoWrapped.tasty` AST address index identifies its direct
`m` definition at `DEFDEF@115`, in the `InfoWrapped` owner. The existing
`four_frozen_members_are_explicitly_completed_in_fresh_deterministic_runs`
test maps that address through the original unit-local index, calls
`TastyUnpickler::complete_symbol`, and records the full completed type. It
also confirms the distinction between source `m: F[Int]` and the serialized
normalized result `ByName(InfoWrapped.F[Int])`.

## Resolver setup and request

`ordinary_infowrapped_m_term_request_returns_the_original_completed_member`
uses `InfoPairAuditClassPath`, the pair loader's existing semantic fixture
path. Its ordered lookup is the original semantic fixture directory first
for `InfoBase` and `InfoWrapped`, then `$JAVA_HOME/jmods` for other classes.
Both selected TASTy resources win from
`crates/dotty-tasty-unpickler/tests/fixtures/semantic`, with
`ClassOrigin::Directory` set to that fixture directory. The test enters the
pair through the existing `resolve_member_with_semantic_info_pair` class-root
route, then obtains the canonical `InfoWrapped` symbol from the resolver
session and builds `Type::type_ref(definitions.no_prefix, wrapped)`. No
`SymbolId` is invented or transferred between stores.

The request passed to the ordinary `SymbolResolver::resolve_member` route is:

```rust
MemberRequest {
    prefix: canonical_info_wrapped_type,
    name: Name::new(store.names.intern("m"), Namespace::Term),
    selector: MemberSelector::Unique,
    space: MemberSpace::Prefix,
}
```

The pair selector handles class roots only. The term request is handled by
ordinary declaration-scope lookup on the already-loaded canonical class; it
does not invoke a classpath fallback or turn a miss into a selected-class
success.

## Measured observations

The focused request runs twice in separate fresh `SemanticStore` and resolver
instances. It normalizes semantic roles and types, not numeric IDs. Both runs
observe the same result:

| Observation | Result |
| --- | --- |
| Member lookup | `Ok(Some(symbol))`; returned ID equals the sole direct `m` declaration in `InfoWrapped`'s declaration scope |
| Identity and ownership | `SymbolKind::Method`, `Namespace::Term`, public, owner is the canonical `InfoWrapped` class |
| Completion | `SymbolInfo::Complete`; serialized method info is directly `ByName(F[Int])` (the by-name node is the root of the normalized info) |
| Type parameters and parent graph | `InfoWrapped` is a class; `InfoBase` is a trait; its one direct parent is `InfoBase[F[Int]]`, with `F` bound to `InfoWrapped.F` and `Int` to the canonical `Int` definition |
| Resource provenance | Both pair roots originate from the original semantic fixture directory |
| Repeat lookup | A second request in the same resolver returns the same member ID |
| Wrong owner | `InfoBase.m` returns `Ok(None)`; the source-authentic member is declared directly on `InfoWrapped` |
| Wrong namespace | `InfoWrapped` type-namespace `m` returns `Ok(None)` |
| Unknown term | `InfoWrapped.unknown` returns `Ok(None)` |

The original-index `complete_symbol` audit independently returns a complete
`ByName(InfoWrapped.F[Int])` type for the indexed `DEFDEF@115`. The pair
resolver observation agrees after normalization. No typed failure occurs in
the successful request, repeat, or ordinary negative controls; therefore
there is no earlier typed failure or fallback error to report.

### B1 decision

**ALREADY_SUPPORTED.** Authentic selected pair loading plus ordinary term
member lookup retrieves the original direct method with completed serialized
type information. No implementation change is warranted in B1.

## Reproduction

Run at test commit
`58cd7780a79c449bfecdff06023a1c93113371c6` or a descendant containing this
test, with the pinned Rust toolchain and a JDK 21 `JAVA_HOME`:

```sh
export JAVA_HOME=/path/to/jdk-21
export PATH="$JAVA_HOME/bin:$PATH"
cargo test -p dotty-classloader --lib ordinary_infowrapped_m_term_request_returns_the_original_completed_member --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --test semantic_tasty_member_completion four_frozen_members_are_explicitly_completed_in_fresh_deterministic_runs --locked -- --nocapture --test-threads=1
cargo test -p dotty-classloader --locked
cargo test -p dotty-tasty-unpickler --locked
cargo fmt --all -- --check
git diff --check
```

Validation on test commit `58cd7780a79c449bfecdff06023a1c93113371c6`:

- `cargo test -p dotty-classloader --lib ordinary_infowrapped_m_term_request_returns_the_original_completed_member --locked -- --nocapture --test-threads=1` — passed.
- `cargo test -p dotty-classloader --test semantic_tasty_member_completion four_frozen_members_are_explicitly_completed_in_fresh_deterministic_runs --locked -- --nocapture --test-threads=1` — passed.
- `cargo test -p dotty-classloader --locked` — passed (unit and integration suites; five existing ignored tests).
- `cargo test -p dotty-tasty-unpickler --locked` — passed (one existing ignored corpus test).
- `cargo test --workspace --all-targets --locked` — passed.
- `cargo fmt --all -- --check`, `python3 tools/check-frontend-boundaries.py`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`, and `git diff --check` — passed; the focused classloader Clippy check also passed at the listed commit.
- The required JDK gate with `DOTTY_CLASSFILE_REQUIRE_JDK=1` — passed using Temurin 21.0.13+9-LTS.

## Hosted validation and review tracking

The PR was opened at
[scytrowski/dotty-rs#2335](https://github.com/scytrowski/dotty-rs/pull/2335).
These hosted workflows were running on PR head
`d9b91df44c35d554ab8a4f173dbde02436f6aaec` when recorded:

- [CI run 43908581367](https://github.com/scytrowski/dotty-rs/actions/runs/43908581367) — in progress.
- [Typer focused audit run 43908581676](https://github.com/scytrowski/dotty-rs/actions/runs/43908581676) — in progress.
- [JVM TASTy factory Scala consumer run 43908581577](https://github.com/scytrowski/dotty-rs/actions/runs/43908581577) — in progress.

Independent review tracking is on the [PR reviews tab](https://github.com/scytrowski/dotty-rs/pull/2335/reviews); review is pending and owned by the orchestrator.

The historical frozen #1927/#1931 14-unit/126-row comparator and #1995
four-method evidence remain unchanged.
