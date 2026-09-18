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
compares normalized token kinds plus source-token start offsets. Colon protocol
variants are normalized to the underlying operator token because the parser
feedback event is not part of this raw differential harness. Scala UTF-16
offsets are converted to Rust UTF-8 byte offsets. Layout tokens are compared
by kind; their exact end offsets differ between the Scala scanner and the
parser-facing Rust stream. A mismatch reports the first token and a source
excerpt around it.

To compare another fixture or corpus directory (recursively):

```text
bash tools/scala-lexer-oracle/compare.sh path/to/fixtures
```

For a real-source corpus where parser feedback is not available, use
`--ignore-layout` to compare only source tokens:

```text
bash tools/scala-lexer-oracle/compare.sh --ignore-layout path/to/corpus
```

The repository's real-source corpus can be checked with:

```text
bash tools/scala-lexer-oracle/check-corpus.sh
```

It covers the Scala sources of the lexer oracle itself and the local
`tasty-baseline` tool. The checked-in fixture directory remains the focused
compatibility matrix; the real-source corpus is a separate smoke test for
tokenization of larger, naturally evolving Scala files.

## Manual CI compatibility check

The GitHub Actions workflow
`.github/workflows/lexer-compatibility.yml` is manual-only, matching the
expensive TASTy compatibility workflow. Run it from the Actions tab and choose
one of these corpora:

- `fixtures` runs the focused Scala 3.9.0 compatibility matrix;
- `source-corpus` runs the larger real-source smoke corpus;
- `all` runs both checks.

The workflow provisions JDK 25, SBT, and Rust, runs the Python normalization
tests, and then invokes the same comparison scripts documented above. Local
SDKMAN initialization remains supported, but CI uses the SBT executable
provided by the workflow setup.
