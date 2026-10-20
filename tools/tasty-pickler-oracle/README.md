# Scala 3.9 pickler oracle

See [the oracle contract](../../docs/tasty-pickler-oracle.md) for pinned cached
dependencies, fixture provenance, snapshot rules, compiler input layout, and
exact opt-in commands.

`run.py check` verifies independently generated fixtures and compiler acceptance;
`run.py verify FILE` validates and compiles a standalone candidate. Add
`--runtime` for supported `example/*.tasty` fixtures to run a Java harness
against the fresh class. The Blocks case verifies ordered block bodies and that
the final expression returns `3`; the structural fixture asserts the preceding
call statements remain in the TASTy stat list. The References case checks
`new References().first() == 42`. The Mutable case verifies ordered local
assignments and signed Int overflow through a fresh JVM. Ordinary
`cargo test -p tasty-pickler-oracle --locked` uses the checked-in fixtures and
environment-independent failure controls without invoking Scala.
