# Semantic TASTy Convergence VII — integrated C1 acceptance

## Scope and provenance

This is the bounded tests/docs/CI-receipt closeout for #2333. No production
semantics, fixtures, historical comparator inputs, or tests were changed in
this PR. The integrated main base tested here is
`82778005f0cf31b523eda6b311f5e71f79fd63b7` (merge of PR #2342).

| Prerequisite | Reviewed PR head | Merge commit | Result |
| --- | --- | --- | --- |
| #2329 audit and #2330 ALREADY_SUPPORTED proof, PR [#2335](https://github.com/scytrowski/dotty-rs/pull/2335) | `54d5ad135b1f632788e9a85aa1e0e2911ec9f25d` | `7b9999c195bf14d20ff73ddfcf670586bc14b722` | reviewed and merged |
| #2331 ordering and canonical identity, PR [#2340](https://github.com/scytrowski/dotty-rs/pull/2340) | `042126bff8e5c8c2ee6ea504c4fe1074d6609d3b` | `50da8d29f933c33ff3ba1451f7655b94797f270f` | reviewed and merged |
| #2332 negative cases and rollback/retry, PR [#2342](https://github.com/scytrowski/dotty-rs/pull/2342) | `9d0dda0ff1edf9a8fbb7b873c2d494aadc8c1a3b` | `82778005f0cf31b523eda6b311f5e71f79fd63b7` | reviewed and merged |

The fixture hashes match the prior A1 audit. `InfoWrapped.m` is the original
`DEFDEF@115`; its completed serialized signature is
`ByName(InfoWrapped.F[Int])`, and `InfoWrapped` directly extends
`InfoBase[F[Int]]`. The classpath test checks the semantic fixture directory
first for both original pair units, records `ClassOrigin::Directory` for each,
and uses the real JDK 21 `jmods` for other classes. The pinned Scala reference
is 3.9.0 source revision
`777528f19a58e794c9954a42f433373472ec57f8`, TASTy minor 28.9. Local validation
used Rust 1.98.1 and Temurin JDK 21.0.13+9-LTS.

Frozen historical inputs were not rewritten:

| Comparator input | Rows (excluding header) | SHA-256 |
| --- | ---: | --- |
| `docs/classloader-semantic-tasty-convergence/cohort.tsv` | 14 | `bfa365d830a627ce46bb5682a93cad90e26dbc9a90ead76bcaa65a48539a8a95` |
| `docs/classloader-semantic-tasty-convergence-ii/member-completion.tsv` | 4 | `5bc658a2c13e35c7751b5ab3a29ed6f0c842b0d0ef1e7bb6468c141a7ad30c61` |

## Executed acceptance

The focused resolver tests passed on the integrated base SHA above and were
also included in the full workspace suite on candidate SHA
`8a6012dfcd7d5ce8f206dfa8c71a7b8d9db25647`. The comparator commands were
rerun twice each on that candidate SHA. That candidate differs from the
integrated base only by this documentation report.
The resolver's positive test creates two independent stores and checks ordinary
term-member resolution, canonical owner and method identity, completed type,
parent application, resource origin, and repeated lookup. The ordering test
runs both valid class request orders in fresh stores and compares normalized
graphs. Negative and transaction tests cover wrong owner/package/namespace,
missing selected resources with source attribution, preservation of a prior
successful class, rollback, and same-session retry.

The `semantic_tasty_member_completion` four-member comparator and
`semantic_tasty_parity` 14-unit/126-field comparator each passed in two
separate invocations on candidate SHA `8a6012dfcd7d5ce8f206dfa8c71a7b8d9db25647`.
Both tests also compare their own fresh internal runs. `acceptance.tsv` records
the concrete test names and results.

Hosted [CI run 44092249958](https://github.com/scytrowski/dotty-rs/actions/runs/44092249958)
passed on candidate SHA `8a6012dfcd7d5ce8f206dfa8c71a7b8d9db25647`: the Rust
checks and Classfile JVM encoder gate both succeeded. The JVM job used the
required JDK 21 gate. The pinned Scala consumer workflow did not trigger for
this documentation-only change; its shard and aggregate results remain
unavailable.

### Status

- **Functional: MET (local integrated tests and hosted Rust/JDK CI).** The positive semantic proof,
  both request orders, repeat lookups, negative controls, and rollback/retry
  all passed on the verified integration base. This does not claim that the
  hosted Definition of Done is complete.
- **Telemetry: PARTIAL.** Local assertions observed fixture resource winners,
  `ClassOrigin`, normalized graphs, typed terminal failures, and rollback
  state. Hosted Rust/JDK 21 receipts are complete on the candidate SHA, but
  pinned Scala 3.9 shard/aggregate receipts are missing. The Scala workflow is
  filtered out for these documentation-only paths. The connected GitHub tools
  available to this worker can read workflow runs but cannot dispatch one; the
  local `gh` CLI is unauthenticated. See the CI link/status column in
  `acceptance.tsv` and the PR checks page for any automatically triggered jobs.

No CI result is inferred from a workflow definition or from prior prerequisite
runs. The missing hosted receipts leave the overall C1 Definition of Done
PARTIAL/UNMET until the applicable jobs pass on an identified SHA.
