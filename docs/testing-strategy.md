# TASTy Decoder Testing Strategy

Every decoder increment must add tests for the complete surface implemented by
that increment before it is committed.

## Required coverage for a new decoder

- one valid test for every supported tag or enum variant;
- valid tests for optional fields, repeated fields, and empty lists;
- malformed or truncated input tests for each structural boundary;
- assertions that the entire bounded payload is consumed;
- integration validation against the Scala 3.9.0 fixtures when the decoder can
  occur in an AST or standard section.
- nested-node and AST-reference resolution tests whenever traversal crosses a
  length-delimited AST payload.

Generic raw decoding should have a tag-matrix test for every tag range that the
raw layer accepts. Semantic decoding may be introduced incrementally, but a
newly supported semantic tag must not be added without its own focused test.

## Fixture-specific coverage

The supplied Scala 3.9.0 fixtures are a compatibility corpus, not only a
collection of smoke-test inputs. Each fixture must have a named integration
test that checks its intended AST surface. At minimum, that test must verify
that all indexed category-five nodes decode to a known structured node and
that the fixture's characteristic tags are present.

Some Scala constructs are encoded through a combination of definition tails,
type trees, and generated nodes rather than one unique category-five tag. Such
fixtures should assert the relevant combination explicitly instead of
inventing a tag-to-language-feature mapping.

Corpus-wide tests remain responsible for file invariants and round-trip
guarantees. They complement, but do not replace, the named fixture tests.

## Baseline corpora

External-library compatibility suites use a corpus directory with a
`manifest.toml` file. The manifest is the source of truth for the baseline
identifier, Scala and TASTy versions, fixture root, and expected inventory
count and byte size. Tests discover `.tasty` files from the manifest-defined
root rather than embedding a library-specific directory walker or inventory
constants in Rust code.

The baseline format is intended to support corpora materialized from a local
directory, a JAR, or a pinned source repository. Regeneration may perform
builds or network access, but ordinary CI verification should operate on the
checked-in materialized files and remain deterministic and offline. Each
baseline should record its source revision or artifact checksum before it is
expanded beyond the initial proof of concept.

The shared test support resolves selection and expectation paths from the
manifest. Semantic-lite JSON is loaded through the same support module for
future corpora, rather than defining a separate schema loader in each library
test. Baseline regeneration is performed with
`tools/tasty-baseline/generate.sh`, which validates `artifact_sha256` before
calling the Scala oracle and replaces the expectation only after successful
generation.

JAR-based manifests may declare `artifact` and `artifact_sha256` together. The
shared manifest parser validates that the pair is complete and that the
checksum is a 64-character hexadecimal SHA-256 digest.

Future semantic expectations belong beside the materialized corpus and must
record the Scala/compiler version and expectation-schema version. They should
compare stable structural or semantic facts, not absolute AST offsets, unless
the offset itself is the behavior under test.

When a fixture has a stable source-level expectation, add focused value
assertions in addition to tag coverage. Prefer one test per independent
expectation (for example, one literal value or one control-flow shape) and
assert names, constants, parameter lists, branch counts, AST-reference
targets, and nested node fields where the public model exposes them. Keep
compiler-generated details such as absolute offsets out of these assertions
unless the offset itself is the behavior under test.

The full suite is the required pre-commit check:

```text
cargo test --all-targets
```
