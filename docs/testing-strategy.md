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

The full suite is the required pre-commit check:

```text
cargo test --all-targets
```
