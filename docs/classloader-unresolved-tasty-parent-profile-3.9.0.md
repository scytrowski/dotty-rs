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

The profile now also runs `ClassLoader::load_class_detailed` for every root.
Full loading ends with `tasty.unresolved_supertype` for 41 roots / 379
observations, `tasty.missing_type_def` for 6 roots / 8,463 observations, and
success for 3 roots / 107 observations. The six missing-type roots are
`dotty/tools/dotc/ast/tpd$` (1,715), `dotty/tools/dotc/ast/untpd$` (141),
`dotty/tools/dotc/config/SourceVersion$` (19),
`dotty/tools/dotc/core/Symbols$` (3,652),
`dotty/tools/dotc/core/Types$` (2,545), and
`dotty/tools/dotc/parsing/Tokens$` (391). Their exact dependency chains and
selected JAR origins are included in the TSV. For example, `tpd$` fails while
loading `Trees$Instance` and then `Trees`; `Symbols$` fails through
`Property$Key` to `Property`. The three full-load successes are
`JavaTokens$` (22), `Action$` (84), and `SymbolInformation$` (1). This
focused classloader path is not identical to the source typer's resolver, so
these outcomes are reported separately from the frozen source-audit count.

The root TSV records every Template parent, not just the first failure. It
separates standalone decoder cause/index from the full-load deepest cause,
failing binary name, dependency chain, and selected resource origin.
`profile_first_unresolved_parent_index` records the independent per-parent
diagnostic pass, which continues after an earlier unresolved parent. Root
artifacts, versions and SHA-256 hashes are retained on each row. The profile
includes stable wire addresses and bounded raw shapes; it does not use arena
IDs or debug formatting. The pinned profile asserts the 41/6/3 full-load
classification as well as the 50-root weighted accounting.

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

All 163 parent rows are available in
[`classloader-unresolved-tasty-parent-profile-3.9.0.tsv`](classloader-unresolved-tasty-parent-profile-3.9.0.tsv), and the source-level root map is in
[`classloader-unresolved-tasty-parent-source-provenance-3.9.0.tsv`](classloader-unresolved-tasty-parent-source-provenance-3.9.0.tsv).
The profile test joins source-audit observations and unique source-file counts
from the retained provenance TSV by requested root as it writes the final
36-column profile. Roots absent from the source map receive `0` observations
and `0` files. This makes the checked-in profile directly reproducible without
rerunning the source corpus audit. Two focused profile runs produced
byte-identical final output (SHA-256
`7157dbe16e1aa8286381276500afafed7c27e52147d69be97a59359cf52a27ac`). Empty
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
`7157dbe16e1aa8286381276500afafed7c27e52147d69be97a59359cf52a27ac`.
