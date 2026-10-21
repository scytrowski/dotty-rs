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
18 compiler roots / 8,590 and 32 library roots / 359. The current pinned
classpath-backed standalone decoder reproduces `tasty.unresolved_supertype`
for 42 roots / 8,929 observations. Eight library roots / 20 observations now
decode successfully with this focused classpath: `scala/collection/IndexedSeq`
(1), `scala/collection/LinearSeq` (1), `scala/collection/View$` (2),
`scala/collection/immutable/IndexedSeq` (4),
`scala/collection/immutable/Seq` (6), `scala/collection/mutable/ArraySeq$`
(2), `scala/collection/mutable/Buffer` (1), and
`scala/collection/mutable/Map` (3). The profile records those as `none`; it
does not rewrite the frozen baseline. This is a precise 20-observation
population difference from the source-corpus audit.

The root TSV records every Template parent, not just the first failure. Its
`reproduced_unresolved_parent_index` and `deepest_typed_cause` columns come from
the normal class/module TASTy decoder. `profile_first_unresolved_parent_index`
records the independent per-parent diagnostic pass, which continues after an
earlier unresolved parent. Root artifacts, versions and SHA-256 hashes are
retained on each row. The profile includes stable wire addresses and bounded
raw shapes; it does not use arena IDs or debug formatting.

The issue baseline reports 279 affected source files, but its frozen root TSV
contains no source-file-to-root rows or request-origin data. This focused run
intentionally does not execute the 1,236-file source audit, so it cannot
reconcile those 279 files or assign source origins per root. The root and
weighted-observation counts are exact; source-file/origin attribution remains
unmeasured here.

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
[`classloader-unresolved-tasty-parent-profile-3.9.0.tsv`](classloader-unresolved-tasty-parent-profile-3.9.0.tsv).
The profile was run twice and compared byte-for-byte. No semantic corpus
baseline or production behavior was changed.

## Reproduction

Run the ignored test with `SCALA39_COMPILER_JAR`, `SCALA39_LIBRARY_JAR`,
`SCALA39_LIBRARY3_JAR`, `JAVA_HOME`, and `SCALA39_PARENT_PROFILE_OUTPUT` set to
the pinned artifacts/JDK. Use `CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1`.
