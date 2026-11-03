# Generated two-field case-class `unapply` identity (Scala 3.9.0)

This document records the initial `Flagged` increment. Issue #2066 replaces
its name/type-spelling gate with the bounded structural rule documented in
[`namer-structural-product2-3.9.0.md`](namer-structural-product2-3.9.0.md).
The generated method remains a canonical synthetic `unapply` Method owned by
the linked companion ModuleClass, indexed by the original class plus the
`Unapply` role, with one synthetic `x$1` formal and no completed signature.

The bounded ordinary shape and explicit companion collision policy are
summarized in the structural document. The pinned Scala 3.9 oracle
classifications for the original Flagged controls remain in
`crates/dotty-typer/tests/fixtures/source-case-class-product2-3.9.0/scala-matrix.tsv`.

At the point recorded by this increment, `_1`/`_2` had not yet been published.
The follow-up selector identity increment is documented in
[`namer-source-case-class-product2-selectors-3.9.0.md`](namer-source-case-class-product2-selectors-3.9.0.md).
Neither increment completes a `MethodType` or types a `Match`. The Typer
fixture's original `rust-baseline.tsv` remains the explicit pre-identity
historical record. The lookup-only result after unapply identity is kept in
`rust-after-unapply-identity.tsv`: extractor lookup reaches the canonical
method, then the existing one-field completion path reports
`MalformedMethodClause`; no typed root is published. Before/after identity summaries are in
[`namer-source-case-class-product2-unapply-3.9.0.tsv`](namer-source-case-class-product2-unapply-3.9.0.tsv).
