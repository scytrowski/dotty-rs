# Enumeration.Value.hashCode acceptance receipt

## Result

The unchanged Scala 3.9.0 source-owned `scala.Enumeration.Value.hashCode`
(`library/src/scala/Enumeration.scala#tree=459`, bytes `9430..9466`) now types
as one complete `DefDef` through lexer, parser, namer and
`SourceTyper::type_method_definition`. The body remains exactly
`override def hashCode(): Int = id.##`; its complete method signature is `Int`.

The focused test initializes two independent parser/namer/store/arena/resolver
sessions. In each session it injects a failure after complete source typing,
checks rollback of the semantic store, typed arena, type index, typed index and
source-to-typed index, then retries the same original method successfully. The
normalized receipts match across sessions and contain no allocation IDs.

The receipt proves canonical provenance at each step:

- `hashCode` is the canonical source method owned by the canonical source class
  `scala.Enumeration.Value`.
- Source tree 457 (`id`, bytes `9461..9463`) maps to canonical source method
  `scala.Enumeration.Value.id`, whose result is `Int`.
- Source tree 458 (`id.##`, bytes `9461..9466`) maps to builtin `Any.##` with
  signature `ByName[Int]`; the typed method's original RHS refers to that
  selected member, and the enclosing complete `DefDef` result is `Int`.
- The typed root and both nested source mappings retain their original spans.

The pinned Scala 3.9.0 compiler oracle accepts the original file and prints
`hashCode(): Int = this.id.##`.

## Cohort evidence

Both frozen 25-root ordinary complete-method audits ran twice with
`--verify-determinism`; each pair of normalized reports matched byte-for-byte,
and both required-cohort gates passed.

| Cohort | Typed | Blocked | Report SHA-256 |
| --- | ---: | ---: | --- |
| Main | 21 / 25 | 4 / 25 | `eb63688c487b1cdece67145304cdd23acbd74584bcfde9a53a73166569fa2e74` |
| Analysis | 17 / 25 | 8 / 25 | `7fcd81ec7bbb0d00b4da439a88fea1d865f31a459a7e7590472de300e23da6ad` |

Compared with the Acceptance VI baseline at merged #2297, all 20 previously
successful main identities and their result types remain unchanged, and the
original `hashCode` root is the one newly typed main identity. All 16 previously
successful analysis identities and their result types remain unchanged;
`PropImpl.clear` also types after its separate prerequisite #2306. The required
main and analysis identity/result gates passed. No frozen manifest or
protected baseline result was edited.

The source revision is `777528f19a58e794c9954a42f433373472ec57f8`. The target
source SHA-256 is
`7370a0d85123fd15c9b75c132a38b911ad71f2a41e851e3121b5a6253939c1e8`. Frozen
main and analysis manifest SHA-256 values remain respectively
`59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244` and
`afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc`.

Integration base `ac6e19bc673f1bddeaf60ec794b6ca5f06c22973` includes prerequisite
merge commits:

| PR | Merge SHA |
| --- | --- |
| #2297 | `0495cb552ebdaf4d3c0555c1eb04e8b035a5debd` |
| #2301 | `292342f66c51f66f4e49e0e12255d385f2f028a0` |
| #2302 | `664f2515746867e37610ec2255172936fd5f7b97` |
| #2306 | `ac6e19bc673f1bddeaf60ec794b6ca5f06c22973` |

## Negative controls and validation

The existing ordinary value-result selection controls remain in place and pass:
`canonical_any_hash_code_does_not_make_a_mutable_int_receiver_stable`,
`source_parameterless_value_selection_keeps_member_argument_and_generic_controls`,
`source_value_result_selection_preserves_member_visibility_across_aliases`,
`unstable_selection_prefix_fails_atomically`,
`bare_field_selection_on_a_mutable_local_still_requires_a_stable_prefix`, and
`by_name_parameter_is_not_a_stable_selection_prefix`. These retain rejection of
unknown/inaccessible members, malformed arguments, wrong visibility/owner
contexts, mutable and by-name prefixes, and unsupported generic-result
stability-dependent prefixes.

Validation also passed the focused source-owned `hashCode` two-session test,
the existing `PropImpl.clear` rollback observer, and the official Scala source
typer oracle. The audit and oracle used the pinned Eclipse Temurin `21.0.13+9`
(feature release 21); its `release`, `bin/java`, and
`jmods/java.base.jmod` hashes match the earlier Acceptance VI receipt. Scala
runtime jars are the existing pinned 3.9.0 artifacts recorded in
[`scala-oracles.tsv`](scala-oracles.tsv).
