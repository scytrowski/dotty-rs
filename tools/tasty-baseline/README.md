# TASTy baseline oracle

This is the Scala 3.9.0 side of the baseline framework. It uses
`scala3-tasty-inspector` to load TASTy files and writes a deterministic JSON
projection containing the input path, tree kinds, and resolved symbol names.

The tool accepts individual `.tasty` files, directories, and JARs. A JAR is
materialized into a temporary directory before inspection. Use
`--classpath=` when the inspected TASTy files refer to class files from a
separate compiler output directory or dependency set.

```text
sbt 'run output.json path/to/classes --classpath=path/to/classes'
sbt 'run output.json path/to/library.jar'
```

To inspect only a deterministic subset, pass a newline-delimited selection
file. Blank lines and lines beginning with `#` are ignored; entries are
matched against the logical paths exposed by a directory or JAR input:

```text
sbt 'run output.json --select=semantic-selection.txt path/to/library.jar'
```

For a checked-in corpus, `generate-all.sh` is the canonical manifest-driven
workflow. It verifies the pinned artifact checksum, materializes the TASTy
entries in a temporary directory, checks the fixture inventory, and generates
both expectations:

```text
tools/tasty-baseline/generate-all.sh \
  crates/dotty-tasty/tests/fixtures/scala3-library \
  path/to/scala-library-3.9.0-bin-SNAPSHOT.jar
```

Use `--refresh-fixtures` to copy the verified `.tasty` entries into the
manifest's `tasty_root`. Use `--check` to regenerate both projections in a
temporary directory and fail if either checked-in expectation differs:

```text
tools/tasty-baseline/generate-all.sh --check \
  crates/dotty-tasty/tests/fixtures/scala3-library \
  path/to/scala-library-3.9.0-bin-SNAPSHOT.jar
```

The artifact must match `artifact_sha256` in the corpus manifest. Updating a
baseline therefore requires deliberately updating the artifact, source
revision, checksum, and generated expectations together. This command is a
developer-side regeneration step; normal Rust CI consumes the checked-in
corpus and does not need network access or SBT.

The script runs SBT in foreground `--server` mode and supplies a temporary
writable `XDG_RUNTIME_DIR`, so it does not depend on an already-running SBT
client or on a pre-existing runtime directory. The execution environment must
still allow local Unix IPC sockets, as required by SBT 2.

The output contains `schema_version = 3` and `scala_version = 3.9.0`. Each file
contains a sorted, duplicate-free `declarations` list with simplified
`TypeDef`, `DefDef`, and `ValDef` identifier names. Operator names are omitted
in this first version, plus basic AST shape counts and a histogram of method
parameter-clause counts. Rust checks declaration-kind presence, that at least
one oracle clause arity is visible in the currently decoded DefDef bodies, and
shape presence; counts are retained for future exact matching because TASTy
may contain additional compiler bookkeeping nodes or traversal boundaries.
Rust also validates declaration uniqueness and kind/name metadata, and requires
the oracle to have at least one matching wire-level shape and parameter-clause
arity. Exact count equality is intentionally deferred until both projections
share the same traversal semantics.
The projection deliberately excludes absolute AST offsets, types,
overload-resolution details, and compiler-generated symbols.
It is a semantic-lite oracle, not a Rust semantic model.

Rust integration coverage discovers every corpus directory below
`crates/dotty-tasty/tests/fixtures` that contains a `manifest.toml`. A new
corpus therefore gets inventory, semantic-baseline, and wire-baseline checks
without adding another Rust test target.

## Wire-aligned Rust baseline

The root Rust package also provides `tasty-wire-baseline`. It reads a
materialized directory of `.tasty` files with the Rust decoder and writes the
manifest-selected `wire-v1` projection. `generate-wire.sh` remains available
as a fast low-level command when the fixture directory is already materialized:

```text
tools/tasty-baseline/generate-wire.sh \
  crates/dotty-tasty/tests/fixtures/scala3-library
```

This projection is deliberately separate from the Scala semantic oracle. It
records exact AST tag counts, definition order, raw definition name references
and kinds, and the wire header sequence of `TYPEPARAM`, `PARAM`, `EMPTYCLAUSE`,
and `SPLITCLAUSE` markers for every indexed definition. Rust integration tests
compare these fields exactly and report the fixture and definition index on a
mismatch. It is a structural regression baseline for a pinned corpus, not a
replacement for an independent semantic oracle.
