# Scala 3.9.0 semantic TASTy convergence baseline

Status: input cohort frozen before results. The original pre-session-contract
run is retained as historical evidence, not as a parity measurement. This
audit observes the existing classloader and semantic unpickler and does not
change production semantics.

## Reproduction environment and policy

- Frozen cohort selection baseline: `2894d8c969e0d210b3f461d35e08d55daf6b2227`.
- Refreshed execution baseline after prerequisite #1940: `7222d2d856aa7b7cab667c9e1e30489c700164cd`.
- Rust: `rustc 1.98.1 (48a229cea 2026-09-01)`.
- Scala reference: 3.9.0 source revision
  `777528f19a58e794c9954a42f433373472ec57f8`.
- TASTy inputs: the legacy classloader validates and accepts 28.8.0 and
  28.9.0; this cohort records the minor version per frozen input. The
  original freeze mislabeled the eight classloader fixtures as 28.8. A
  pre-measurement header check corrected only those version fields to 28.9;
  all original unit IDs, paths, hashes, requested shapes, ownership rationale
  and cohort order are unchanged. No adapter result informed this correction.
- Bootstrap policy: independently create a fresh `SemanticStore` per adapter
  and per run, call `Definitions::bootstrap` once, use the same ordered
  composite classpath for both adapters, and give the unpickler a fresh
  `TastySession` with the existing `ClasspathSymbolResolver`. This exercises
  the resolver-backed session contract added by #1940. Do not add semantic
  stand-ins for absent external dependencies; classify those resolutions
  explicitly.
- Cohort was selected and hashed before adapter results were collected. The
  exact input paths, hashes, requested shapes, and rationale are in
  [`cohort.tsv`](classloader-semantic-tasty-convergence/cohort.tsv).
- The comparison TSV schema is frozen in
  [`comparison.tsv`](classloader-semantic-tasty-convergence/comparison.tsv).

## Comparison contract

For each selected unit, report owner path, kind, name, visibility, flags,
class hierarchy, ordered type parameters and parameter clauses,
method/result/field type structure, companion relationship, and only the
annotation forms explicitly supported by both adapters. Normalize type and
symbol identities within each store to owner-qualified paths and semantic
shape. Never compare numeric IDs or TASTy addresses across stores, and never
resolve production identities by rendered names. Each field receives exactly
one outcome: `MATCH`, `SEMANTIC_DELTA`, `LOADER_UNSUPPORTED`,
`UNPICKLER_UNSUPPORTED`, `MALFORMED_OR_VERSION`, or `UNRESOLVED_EXTERNAL`.
Retain the first underlying error and adapter provenance. An absent or
unknown value is not `Any`, `Object`, success, or structural equality.

The audit test validates manifest uniqueness, fixture hashes, format versions
and root coverage, and emits normalized TSV twice from fresh sessions. The
checked-in TSV is the measurement from the refreshed execution baseline;
execution fails on missing/duplicate keys, skipped units, malformed rows, or
outcome mismatch. The prior resolver-free output is retained only as historical
evidence and is not used as parity evidence.

## Refreshed measurement and scope gate

The 2026-11-01 execution used Rust `1.98.1`, refreshed main
`7222d2d856aa7b7cab667c9e1e30489c700164cd`, and
`JAVA_HOME=/home/vdrake6/.sdkman/candidates/java/current`. The real
`jmods/java.base.jmod` input had SHA-256
`fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91`. The
same ordered composite classpath (three classloader fixture roots, the pinned
Scala library fixtures, the path-dependent fixture root, then the JDK) was
given independently to the legacy loader and a resolver-backed `TastySession`.
Each adapter used its own fresh store and one bootstrap. The unpickler resolver
was carried through session handoffs; no semantic stand-ins were inserted.

The original single-unit candidate `visibility-open` and two-unit candidate
`animal` + `dog` were retained. The legacy loader returned all nine fields for
`visibility-open`; all nine unpickler fields stopped at its first actual
`UnresolvedMember` (`java.lang.Object`, TASTy address 15). For both `animal`
and `dog`, the unpickler likewise stopped at the first `UnresolvedMember`
(`java.lang.Object`, addresses 9 and 15). The legacy loader completed the
hierarchy for both units, while each had one explicitly unsupported member
signature caused by the unresolved `scala/Predef/String` reference. Thus the
single-unit and two-unit cases do not qualify as interoperable integration
candidates under the frozen contract.

Across all 14 frozen rows, 13 files have a matching entered root definition;
the `visibility-holder` file has no root named `Holder` in either adapter.
This leaves more than the required 12 real root units without substituting a
candidate. The remaining cohort also yields explicit unsupported or unresolved
outcomes, including `scala.Nothing`, `scala.Int`, and the applied `scala.Tuple2`
parent shape. The per-field report contains 126 rows and is byte-identical
across two fresh sessions. The raw execution trace SHA-256 is
`1b4280c931e0d53e8d4fd03c8e4e2fc4934d14a34491013c7e55b9982983b2ac`; the
normalized comparison TSV SHA-256 is
`6065fa5947a988166831434fd01610d5f367646559b9c34ea75139cdcbc36eba`.

Neither frozen integration candidate qualifies, as permitted by the issue's
DoD when the report explains why no candidate qualifies. The negative controls
reject a one-byte truncation of a real TASTy fixture, confirm a missing JDK
dependency is explicit and rolls back classloader semantic allocations, then
successfully load an unrelated real fixture with a complete classpath in the
same store. The existing unpickler identity regression
`references_to_same_named_classes_resolve_by_address_not_by_name` verifies two
real `Inner` classes remain owned by distinct `Left` and `Right` classes; the
unpickler's `unmodelled_types_are_explicitly_unsupported` regression covers an
unknown semantic type shape. The frozen `scala/Tuple2` applied-parent case
retains the legacy loader's `UnresolvedSupertype` and the unpickler's first
unresolved `scala.Nothing` reference, neither of which counts as equality or a
supported shape. The unpickler regression
`a_failed_decode_takes_back_everything_it_allocated` checks its store rollback.
The two-adapter matrix runs twice in independent fresh sessions and requires
byte-identical normalized output.
Making the frozen integration candidates complete would require changes to
external semantic resolution or its bootstrap contracts, beyond this audit's
tests/tooling/docs scope. The original pre-resolver run remains preserved at
`target/parity-run.txt` in the prior #1927 worktree with SHA-256
`61a7f0e069c46ee0b492da9e8b78ed7347f7439ab2f9f93afe7e2771a7c9c448`.
This boundary is also explicit in the
[`#1940 session contract`](classloader-semantic-tasty-session-contract.md),
which leaves external aliases and the `Open.tasty` and Animal/Dog graph
unsupported by the resolver handoff.
