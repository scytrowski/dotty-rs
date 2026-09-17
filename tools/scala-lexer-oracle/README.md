# Scala lexer oracle

This tool exposes the raw token stream produced by the Scala 3.9.0 compiler
scanner. It is a development oracle for `dotty-lexer`, not a production
dependency of the Rust workspace.

The tool is intentionally pinned to:

- Scala 3.9.0;
- JDK 25;
- sbt 2.0.9.

Run it with SDKMAN's selected defaults:

```text
source "$HOME/.sdkman/bin/sdkman-init.sh"
sbt --error 'run path/to/input.scala'
```

The output is tab-separated and has one row per compiler token:

```text
token\tstart\tend\tline_start\tname\tstring_value\tbase
```

The final `EOF` row is included. Token ends are the start of the following
scanner token, which also makes zero-width compiler-inserted tokens visible.
Fields are escaped using a small JSON-style escape set (`\\`, tab, newline,
carriage return, and backslash) so the output can be consumed by scripts
without an additional dependency.

## Differential comparison

The repository includes a small developer harness that compares the
parser-facing Rust scanner with the Scala 3.9.0 oracle on the fixture set:

```text
bash tools/scala-lexer-oracle/compare.sh
```

The harness runs the pinned sbt oracle, runs the Rust scanner dump example, and
compares normalized token kinds plus source-token start offsets. Scala UTF-16
offsets are converted to Rust UTF-8 byte offsets. Layout tokens are compared
by kind; their exact end offsets differ between the Scala scanner and the
parser-facing Rust stream. A mismatch reports the first token and a source
excerpt around it.

To compare another fixture directory:

```text
bash tools/scala-lexer-oracle/compare.sh path/to/fixtures
```
