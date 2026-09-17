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
