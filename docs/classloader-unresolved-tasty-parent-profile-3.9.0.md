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

The saved #1342 audit report confirms 8,949 observations over 50 root targets
and 279 affected source files. Its normalized summary does not preserve a
complete source-file-to-root relation. The focused profile preserves exact
root artifact provenance and actual classloader dependency origins, but cannot
assign all 279 source files or independent source request origins to roots.
The 279-file aggregate reconciles to the frozen baseline; per-root source
attribution is an explicit remaining limit of the saved evidence, not a claim
that the missing relation was reconstructed.

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
The two focused runs produced byte-identical output (SHA-256
`a9b0a37b57fd9dd91dc5f6c08ae09088a34d7cf77ff16e63aa7e59934c8a326c`). Empty
TSV cells use `-`. No semantic corpus baseline or production behavior was
changed.

## Reproduction

Run the ignored test with `SCALA39_COMPILER_JAR`, `SCALA39_LIBRARY_JAR`,
`SCALA39_LIBRARY3_JAR`, `JAVA_HOME`, and `SCALA39_PARENT_PROFILE_OUTPUT` set to
the pinned artifacts/JDK. Use `CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1`.
