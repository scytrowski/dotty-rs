# Scala 3.9 pickler oracle

See [the oracle contract](../../docs/tasty-pickler-oracle.md) for pinned cached
dependencies, fixture provenance, snapshot rules, compiler input layout, and
exact opt-in commands.

`run.py check` verifies independently generated fixtures and compiler acceptance;
`run.py verify FILE` validates and compiles a standalone candidate. Ordinary
`cargo test -p tasty-pickler-oracle --locked` uses the checked-in fixtures and
environment-independent failure controls without invoking Scala.
