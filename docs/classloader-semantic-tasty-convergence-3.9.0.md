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
  28.9.0; this cohort records the minor version per frozen input.
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

The audit test validates manifest uniqueness, eligibility, fixture hashes,
format versions and completeness, and emits normalized TSV twice from fresh
sessions. The checked-in TSV is the measurement from the refreshed execution
baseline; execution must fail on missing/duplicate keys, skipped units,
malformed rows, or outcome mismatch. The prior resolver-free output is not
used as parity evidence.
