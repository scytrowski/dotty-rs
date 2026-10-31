# Inherited instance captures in ordinary `Function1` literals

The Typer's ordinary one-parameter `Function1` capture path recognizes a
public, immutable inherited member when its declaring class is the enclosing
source class's single direct class parent. It resolves the declaration through
the existing inherited-member lookup and captures the lexical enclosing
instance (`Child.this`). The selected member retains its canonical parent
symbol. This does not add trait linearization, generic parent adaptation,
nested lambda capture, writes, or external hierarchy traversal.

Focused source-Typer evidence is in
`typer::tests::ordinary_function1_owner_resolves_inherited_method_and_field_before_capture`
and `typer::tests::enclosing_this_capture::same_owner_member_capture_fails_closed_and_rolls_back_rejected_lambdas`.
Both roots produce complete method definitions and one `ThisType` receiver in
the closure environment; wrong argument types and mutable writes continue to
reject transactionally. The pinned Scala oracle was not run in this workspace:
the Scala checkout and JDK 21 `java` executable were unavailable.

`tests/fixtures/inherited-members-3.9.0/four-root-baseline.tsv` retains the
before-state evidence. The issue-local after-state summary is in
`tests/fixtures/inherited-members-3.9.0/inherited-capture-after.tsv`.
