# Selected `Option.getOrElse` member projection (#1930)

This supplemental result measures one ordinary member in the unchanged frozen
cohort. It does not change the historical 14-unit / 126-row reports.

| Evidence | Result |
| --- | --- |
| Fixture | `crates/dotty-tasty/tests/fixtures/scala3-library/scala/Option.tasty` |
| SHA-256 | `3db4c542a097dfc91e0fd92e0726070cfcde00e32f312b37b5b6d9383475d20d` |
| Owner and member | `scala.Option@507.getOrElse@666` |
| Method type parameter | `B@669`, recovered from the original AST address index |
| Default parameter | `default@681`, recovered from the original AST address index |
| Default wire shape | `BYNAMETPT@94` around the direct reference to the method-local `B` |
| Result type tree | address `687`, referring to the same method-local `B` |
| Pinned source | Scala 3.9.0 `Option.scala` at `777528f19a58e794c9954a42f433373472ec57f8`: `getOrElse[B >: A](default: => B): B` |

Before the change, the legacy member projection represented `default` as
`Error(unresolved .tasty member type: TASTy decoder could not reduce the
declared type tree to a supported shape)` and the result as `ParamRef[0]`.
Explicit member completion through the canonical resolver-backed unpickler
already succeeded with
`[B:None:bounds(scala.Option.A,Any)]=>(by-name(ParamRef[0]))Plain:ParamRef[0]`.
The implementation adds support only for `BYNAMETPT(ParamRef)` in this member
type decoder and lowers that shape to `Type::ByName` around the existing
method-binder reference.

The focused decoder test asserts the two frozen absolute addresses and keeps
the original address-index mapping. It rejects a same-spelled `B` from a
different method and an unmatched/dangling direct reference. The focused
loader test lowers the actual decoded `getOrElse` member and checks its
polymorphic bound against the canonical `Option.A` symbol, ordered `default`
clause, method-local binder identity in both parameter and result, public
visibility, and decoded flags.

The live comparator ran twice with fresh adapter sessions and retained the
same 126 row keys. Exactly one legacy value differs from the historical
`comparison-1948.tsv`: `option/member_signatures`. Its overall outcome remains
`LOADER_UNSUPPORTED` because the row aggregates unsupported neighboring
members. No other cohort result changed. The frozen cohort and both checked-in
historical comparison TSVs retain their original hashes.
