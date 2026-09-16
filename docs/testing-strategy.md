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
