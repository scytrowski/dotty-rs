# Scala 3.9 `PropImpl.clear` complete-method acceptance

## Result

The unchanged Scala 3.9.0 `scala.sys.PropImpl.clear` source method types as a
complete source-owned `DefDef` in two independently initialized Rust frontend,
resolver, semantic-store, and arena sessions. Both normalized semantic receipts
match. The method's declared and typed result is `Unit`; its source RHS remains
the original `underlying -= key` operation.

The implementation under test is PR commit
`39cd81451b9c236db965f724ea4c5ba1078be423`, reachable from the PR head. The
focused PropImpl acceptance and negative/rollback tests were rerun on that
exact commit. No production typing logic, cohort manifest, protected result,
or release threshold changed.

## Source and typed-tree evidence

The source is pinned to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. Its SHA-256 is
`3e9946686782a9af7259a130cf8ee7467cac28a91469c0e3f59c5fc778c6157e`, matching
the existing Acceptance VI source pin. Root identity is
`library/src/scala/sys/PropImpl.scala#tree=99`, canonical owner
`.scala.sys.PropImpl`, source line 43, and byte range `1297..1334`. The original
RHS is tree 98 at `1317..1334`; its `underlying` receiver is tree 96 at
`1317..1327` and `key` is tree 97 at `1331..1334`.

The focused Rust test calls `SourceTyper::type_method_definition` directly
without caller-injected `complete_symbol`, manual member preparation, or
priming. It verifies the canonical `scala.collection.mutable.Shrinkable.-=`
member, the original receiver and argument mappings, and the inner operation's
unchanged `Shrinkable` result. The typed RHS is the expected `Unit` adaptation:
one statement containing that application, followed by the canonical `()`
literal. Every positioned source node within the original method span maps to a
typed node. The normalized receipt records:

```text
root=PropImpl.scala#tree=99 span=1297..1334 owner=.scala.sys.PropImpl result=Unit rhs=1317..1334 operation=Shrinkable.-= operation_result=Shrinkable receiver=tree96 argument=tree97 adaptation=Block(statements=1,final=()) mappings=95:1310..1314,96:1317..1327,97:1331..1334,98:1317..1334,99:1297..1334
```

The pinned Scala compiler oracle was run with `-Yexplicit-nulls
-Ystop-after:typer -Xprint:typer`; its tree is `clear(): Unit = { this.underlying
-= PropImpl.this.key; () }`. The normalized oracle log SHA-256 is
`045a5afe78a7019463b93b7c746c293ac7f0fade997a03326b0db847b99fa04e`.

## Negative cases and rollback

The focused negative matrix rejects each input before publishing a typed root
or mappings:

| Case | Expected rejection |
| --- | --- |
| `underlying -= 1` (wrong operand type) | `OverloadApplicationNoApplicable` |
| `missing -= key` (missing receiver) | `TermNameNotFound` |
| `42 -= key` (incompatible receiver) | `MemberNotFound` |
| `underlying.noSuchMember(key)` | `MemberNotFound` |
| declared result changed from `Unit` to unrelated `Int` | `ExpectedExpressionTypeMismatch` |
| original source root presented under a different `SourceId` | no canonical source symbol; rejected before typing |

The natural late-error control confirms that the original operation types
before an unresolved final identifier fails, then verifies rollback of the
semantic store, typed arena, type index, typed index, source-to-typed index, and
committed root. Separately, the unchanged original method is run with the
existing injected late-failure hook. The test verifies the same rollback
checkpoints, then disables the automatically consumed injection and retries the
original `DefDef` through the same `SourceTyper` and its retained resolver.
That retry asserts the canonical method signature and typed root mapping, the
single original `Shrinkable.-=` application, its enclosing `Unit` block, and
the final canonical `()` literal.

## Frozen ordinary cohorts

Both existing 25-root manifests were left byte-identical and rerun in ordinary
complete-`DefDef` mode with `--verify-determinism` on tested commit
`39cd81451b9c236db965f724ea4c5ba1078be423`. Each determinism run compared two
fresh audit sessions and produced identical normalized results.

| Cohort | Manifest SHA-256 | Roots | Typed | Blocked | Report SHA-256 |
| --- | --- | ---: | ---: | ---: | --- |
| main | `59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244` | 25 | 21 | 4 | `eb63688c487b1cdece67145304cdd23acbd74584bcfde9a53a73166569fa2e74` |
| analysis | `afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc` | 25 | 17 | 8 | `7fcd81ec7bbb0d00b4da439a88fea1d865f31a459a7e7590472de300e23da6ad` |

The protected 16-root main and 15-root analysis result gates passed using the
unchanged `required-main.tsv` and `required-analysis.tsv` files. Their SHA-256
values remain `05cdcf0f9185a32075ca8f6bfd3a37084e4127384c0940c9783ae2626dc9377f`
and `021bb2d5a1e26ea575bf3f89b92c5a5e41acc2e124e753e43715608f0fe4bce4`.
Analysis `PropImpl.clear` is now `typed`, source tree 99 to typed root 7, with
expected result `Unit` and actual result `Unit`. The main cohort's protected
successes and results remain intact.

## Toolchain provenance

The JDK is Eclipse Temurin `21.0.13+9`. Hashes for its `release`, `bin/java`,
and `jmods/java.base.jmod` are respectively:

```text
f83f6dcc966043c93cd498ac8e7ee7c310852074970d62c702e4ad2710002898
9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155
fad51abeafe6a1ecb0c2696a88684dc417cc534b74b5d865042df7381d9d0e91
```

The pinned Scala classpath artifact hashes are:

| Artifact | SHA-256 |
| --- | --- |
| `compiler-interface-1.12.0.jar` | `92f487a505eafdd0a033e4ff4f38a81509a655c76257c64e6808dd873cc145c3` |
| `scala-asm-9.9.0-scala-1.jar` | `75ac366e8ecb691e06a7e85041eed0f67919a646e5262fa0901225698c104375` |
| `scala-library-3.9.0.jar` | `b844bf6fb3080c73df6b9c523b7a69b9ef4a980bb15e5c37c35443d1d0b0e0cd` |
| `scala3-compiler_3-3.9.0.jar` | `3e2c09a5ca6cef17a55f2f66e123815f9c4ea061fce20d0e9d2371989405a61a` |
| `scala3-interfaces-3.9.0.jar` | `64dfde1d2c6309d65a01c5f3ea836137eb84338dcadeb16f85764561ef4e10db` |
| `scala3-library_3-3.9.0.jar` | `9343f9651453945fc6a9b75bb4fd230418ee28a1f88ea28ee429f3ebe0511aba` |
| `tasty-core_3-3.9.0.jar` | `66e12dda98bb23e864f3a31f8527c8338bf47bce9b85cc380b368c69c1ba8d0f` |
| `util-interface-1.11.5.jar` | `1fc950a9e1a35385e79316e23c120d4fd184f6a7af0274e00a703b0dc76376af` |

## Validation

Passed on the test implementation:

- `cargo test -p dotty-typer --lib propimpl_clear_ --locked -- --ignored --nocapture`
- `cargo test -p dotty-typer --all-targets --locked`
- `cargo test --workspace --all-targets --locked` with Temurin 21 on `PATH`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`
- `python3 tools/check-frontend-boundaries.py`
- `git diff --check`

The ordinary audit commands use the two manifests above, set
`DOTTY_AUDIT_TYPE_METHOD_DEFINITION=1`, and pass their corresponding protected
cohort file with `--require-typed-cohort`. All generated reports and Scala
oracle logs were kept in the registered managed scratch directory and are
summarized here by their hashes.
