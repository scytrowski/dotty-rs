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
`Loop.tasty` verification compiles a Rust-produced `while` loop and checks
`sumTo(0)`, `sumTo(1)`, `sumTo(5)`, and `sumTo(-1)` in the fresh JVM. Ordinary
`cargo test -p tasty-pickler-oracle --locked` uses the checked-in fixtures and
environment-independent failure controls without invoking Scala.

The strict linked-binary consumer gate is:

```sh
DOTTY_REQUIRE_SCALA_TASTY_CONSUMER=1 \
  cargo test --test jvm_tasty_scala_consumer --locked -- --nocapture
```

It requires `SCALA39_ROOT`, `JAVA_HOME`, and `COURSIER_MAVEN_ROOT`, verifies
their pinned revisions/hashes, consumes two identical fresh Rust-emitted pairs,
and runs the named-argument Scala client under JDK 21 `-Xverify:all`. The
client reads both constructor values (`7` and `2`) before checking the state
sequences `7 → 13 → -42` and `2 → -42`.
Its JSON report includes the source fixture hash, both class/TASTy hashes and
the link UUID. The compile-only control removes `get` from the classfile but keeps its TASTy
sidecar; Scala must still resolve it. The same classfile without its sidecar
and a classfile with a mismatched link UUID must fail their controls. Before
compilation, the harness decodes the class-level `TASTY` attribute and TASTy
header and requires their UUIDs to match; renaming the attribute to `XASTY`
must be rejected by the artifact-validation control.
