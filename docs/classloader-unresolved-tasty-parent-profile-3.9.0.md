# Scala 3.9.0 unresolved TASTy parent profile

This is a bounded diagnostic of the 50 frozen resource roots in the post-#1342
baseline. It reads the exact `.tasty` entries from the pinned Scala 3.9.0
compiler/library JARs, checks their archive origins, and runs with one Cargo
job and one Rust test thread. The opt-in ignored test is
`profile_frozen_unresolved_supertype_parent_roots` in
`dotty-classloader/src/tasty_symbol.rs`; it does not type the source corpus or
change production parent-resolution behavior.

## Reconciled population

The frozen root list still accounts for 50 roots / 8,949 weighted observations:
18 compiler roots / 8,590 and 32 library roots / 359. Standalone decoding of
the requested resource reproduces `tasty.unresolved_supertype` for 42 roots /
8,929 observations. Eight library roots / 20 observations decode directly;
this alone does not mean that loading their dependency graph succeeds.

The profile also runs `ClassLoader::load_class_detailed` for every root. The
post-#1414 run compared with the checked-in pre-#1414 snapshot moves
`tasty.unresolved_supertype` from 41 roots / 379 weighted observations to 27
roots / 124 observations: 14 roots / 255 weighted observations recovered,
with no newly unresolved roots. The full-loader outcomes are now
`tasty.missing_type_def` for 7 roots / 8,560 observations,
`classloader.not_found` for 7 / 28,
`tasty.unresolved_supertype` for 27 / 124, and success for 9 / 237. The seven
missing-type roots are
`dotty/tools/dotc/ast/tpd$` (1,715), `dotty/tools/dotc/ast/untpd$` (141),
`dotty/tools/dotc/config/SourceVersion$` (19),
`dotty/tools/dotc/core/Symbols$` (3,652),
`dotty/tools/dotc/core/Types$` (2,545), and
`dotty/tools/dotc/parsing/Tokens$` (391), and `scala/Predef$` (97). Exact
dependency chains and selected JAR origins are included in the TSV. For
`tpd$` (1,715) and `untpd$` (141), the applied parent now resolves to the
verified `dotty/tools/dotc/ast/Trees$Instance`; loading continues to `Trees`
and stops there on `tasty.missing_type_def`. These roots therefore demonstrate
the parent-resolution change, not complete class loading. The full-loader
profile is not identical to the source typer's resolver, so the two outcome
sets remain separate.

The root TSV records every Template parent, not just the first failure. It
separates standalone decoder cause/index from the full-load deepest cause,
failing binary name, dependency chain, and selected resource origin.
`profile_first_unresolved_parent_index` records the independent per-parent
diagnostic pass, which continues after an earlier unresolved parent. Root
artifacts, versions and SHA-256 hashes are retained on each row. The profile
includes stable wire addresses and bounded raw shapes; it does not use arena
IDs or debug formatting. The pinned profile asserts the updated per-root
movement (41 to 27 unresolved roots, 379 to 124 weighted observations), the
full-load outcome classification, and the 50-root weighted accounting.

A single instrumented source audit pass now records the source-file-to-root
relation for all 50 frozen roots. It found 8,948 unresolved-supertype
observations across the same 279 source files. The 347-row provenance TSV
contains the root cause key, requested root, source path, and observation
count; the profile TSV adds per-root observation and unique-file counts to
every parent row. This reconciles the root and file populations while exposing
a one-observation total drift from the frozen 8,949 baseline. Six roots have
changed weights: `Symbols$` 3,652→3,673, `Types$` 2,545→2,539, `tpd$`
1,715→1,719, `Tokens$` 391→375, `retainsCap` 115→114, and `ListBuffer`
21→18. We retain the original baseline and report the measured current pass
separately; no expected data was regenerated.

## Failure families and follow-up ranking

Ranking is by weighted observations in the frozen baseline, with the actual
wire shape and resolution boundary retained:

1. **`Symbols$` / `Types$`: 6,197 observations across two roots.** The first
   failing parents are `SELECTin` (`0x75`) trees for `SymUtils` and `TypeUtils`,
   each with an explicit `dotty/tools/dotc/core` prefix. The raw parents are
   zero-argument `Apply` wrappers. These are strong candidates for one narrow
   selected-parent projection family. Verify owner identity and module-vs-class
   identity against the pinned source before accepting a candidate.
2. **`tpd$`: 1,715 observations.** Its first parent is a nested
   `Apply(TypeApply(SelectIn(...)))` shape named `Instance`, with visible
   `Trees`, `Type`, and package names. The following `TypedTreeInfo` parent
   resolves. Treat the first parent as a nested `Trees.Instance[Type]`-like
   constructor; preserve the owner/type-argument distinction rather than
   concatenating visible names. The wire prefix is not fully reconstructed by
   this profile, so this diagnosis is shape-grounded, not a claim of a proven
   binary target.
3. **Remaining compiler roots: 678 observations.** These span multiple
   selected, package-qualified, applied and unsupported shapes. Keep them
   separate until focused source-oracle fixtures establish which have a
   trustworthy owner path. In particular, `Tokens$` (391) and `untpd$` (141)
   are separate roots, not evidence for a shared fix.
4. **Library roots: 359 observations in the frozen baseline.**
   `retainsCap` (115) has an unresolved selected `Annotation` first parent,
   followed by `StaticAnnotation`, which the classpath candidate resolver
   finds as `scala/annotation/StaticAnnotation`. `Predef$` (97) starts with a
   selected `LowPriorityImplicits` parent under `scala`. The standalone run
   reproduces 339 library observations; the 20-observation difference is the
   eight successfully decoded collection roots listed above. Package-object
   aliases and bare classpath candidates should remain distinct follow-up
   slices; the profile's `alias_owner` / `alias_name` columns show where an
   actual alias was identified.

All 163 post-#1414 parent rows are available in
[`classloader-unresolved-tasty-parent-profile-3.9.0-post-1414.tsv`](classloader-unresolved-tasty-parent-profile-3.9.0-post-1414.tsv).
The pre-#1414 comparison snapshot used by the profile test remains in
[`classloader-unresolved-tasty-parent-profile-3.9.0.tsv`](classloader-unresolved-tasty-parent-profile-3.9.0.tsv),
and the source-level root map is in
[`classloader-unresolved-tasty-parent-source-provenance-3.9.0.tsv`](classloader-unresolved-tasty-parent-source-provenance-3.9.0.tsv).
The profile test joins source-audit observations and unique source-file counts
from the retained provenance TSV by the `::requested=` identity (not the
deepest dependency in `::root=`) as it writes the final 36-column profile. It
asserts exact coverage of all 50 frozen roots and that the joined observations
sum to 8,948 across 279 source files. This makes the checked-in profile
directly reproducible without rerunning the source corpus audit. Two focused
profile runs produced byte-identical final output (SHA-256
`7b74d1f4bfdb5ea8b5df5802fff7900e9a93e71495476cdccaa1afc6cb7b9c9d`). Empty
TSV cells use `-`. No semantic corpus baseline or production behavior was
changed.

## Reproduction

Run the ignored test twice with `SCALA39_COMPILER_JAR`,
`SCALA39_LIBRARY_JAR`, `SCALA39_LIBRARY3_JAR`, and `JAVA_HOME` set to the
pinned Scala 3.9.0 artifacts and Temurin 21.0.8. Set the output path separately
for each run; the test joins the retained source-provenance TSV itself:

```sh
export SCALA39_COMPILER_JAR=/path/to/scala3-compiler_3-3.9.0.jar
export SCALA39_LIBRARY_JAR=/path/to/scala-library-3.9.0.jar
export SCALA39_LIBRARY3_JAR=/path/to/scala3-library_3-3.9.0.jar
export JAVA_HOME=/path/to/temurin-21.0.8+9

CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
SCALA39_PARENT_PROFILE_OUTPUT=/tmp/parent-profile-a.tsv \
cargo test -p dotty-classloader --lib --locked \
  profile_frozen_unresolved_supertype_parent_roots -- --ignored --nocapture

CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
SCALA39_PARENT_PROFILE_OUTPUT=/tmp/parent-profile-b.tsv \
cargo test -p dotty-classloader --lib --locked \
  profile_frozen_unresolved_supertype_parent_roots -- --ignored --nocapture

cmp /tmp/parent-profile-a.tsv /tmp/parent-profile-b.tsv
sha256sum /tmp/parent-profile-a.tsv /tmp/parent-profile-b.tsv
```

Both files should hash to
`7b74d1f4bfdb5ea8b5df5802fff7900e9a93e71495476cdccaa1afc6cb7b9c9d`.
