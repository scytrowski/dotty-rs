# Acceptance VII baseline: `Predef.SeqCharSequence.charAt`

**Outcome: PROVISIONAL.** The measurements below are pinned to integrated
`main` at `bdc41b213bd94cdeb239fa7f58fb5ec412cd46ca`. #2308 is merged into
this history and closed, but its closeout explicitly leaves the Acceptance VI
gate unmet pending a successful hosted focused-audit run on the exact reviewed
head. No such green hosted receipt is established here, so the 21/25 and 17/25
counts are measurements, not a signed-off official VII baseline.

## Frozen inputs and measurements

The existing ordinary-method runner was used with the two unchanged 25-root
manifests. Each cohort was run twice independently; each run also used the
runner's two-session determinism check. All five pairs of normalized reports
were byte-identical. Each main run attempted 25/25 roots (21 typed, 4 blocked);
each analysis run attempted 25/25 (17 typed, 8 blocked). No roots were skipped.
All 21 previously typed main roots and all 17 previously typed analysis roots
retain their exact owner and result shape. The per-root records, including
canonical signature shape and source-to-typed root mappings for every success,
are in [`roots.tsv`](roots.tsv).

The main manifest SHA-256 is
`59bb65f48e7d94423de0f0a00029b98a0b41583e601c700687311e9e17fc5244`; the
analysis manifest SHA-256 is
`afa5a2064efd0bfcd31bb0bcb4fb08030db2d180a7619ac3a0843ebbe867f7bc`. The
frozen files were not changed. The #2308 closeout records its previous
integrated result as 21/25 and 17/25 and reports the exact hosted-head gate as
pending; this run confirms the counts and protected results on the newer base.

## Exact original root and first blocker

The audited root remains the unchanged source-owned
`library/src/scala/Predef.scala#tree=614`, owner
`scala.Predef$.SeqCharSequence`, source method `charAt(index: Int): Char`,
source range `20020..20096` (line 457). The first error is
`TermNameNotFound` on the unqualified identifier `sequenceOfChars`, source
tree 611, span `20074..20089`, before any `IndexedSeq.apply` lookup. The
ordinary audit reports expected result `Char`, actual result unavailable, and
no nested cause.

The existing test-only semantic observation maps the owner to class tree 637
and the plain constructor parameter to source tree 602, constructor tree 635,
and a direct class-owned `Parameter` symbol. Its source declaration is
`sequenceOfChars: scala.collection.IndexedSeq[Char]`; the indexed symbol has
`Private` visibility and `SymbolInfo::Missing`. It is not a source `val`
accessor. `charAt` is tree 614, with its canonical class owner. The failing
expression is an `Apply` whose function is the bare identifier; there is no
explicit receiver or selection tree at the failure point. Thus the first
blocker is constructor-parameter term resolution, not external `apply`
selection or result conformance. Numeric arena and symbol IDs are local to the
observation session; stable source tree identities and the exact failure are
recorded in [`first-blocker.tsv`](first-blocker.tsv).

## Scala 3.9.0 oracle and controls

The unchanged upstream `Predef.scala` at Scala source revision
`777528f19a58e794c9954a42f433373472ec57f8` was compiled with the official
Scala 3.9.0 compiler using `-Ystop-after:typer -Xprint:typer`. The oracle
passes; its typed tree shows
`SeqCharSequence.this.sequenceOfChars.apply(index)` with result `Char`.
This uses the original source file directly, not a rewritten replacement.

Pinned controls also pass for a plain constructor parameter inside a method,
a `val` constructor parameter, a same-name method parameter shadowing the
constructor parameter, and `IndexedSeq[Char].apply(Int)`. The wrong-scope
control fails as expected with Scala diagnostic E006 (`sequenceOfChars` not
found). Control sources and command/output hashes are recorded under
[`oracle-controls/`](oracle-controls/) and in [`oracles.tsv`](oracles.tsv).

## Provenance and reproduction

Scala 3.9.0 source revision:
`777528f19a58e794c9954a42f433373472ec57f8`. Scala JARs, source hash, JDK
release/java/base-module hashes, and normalized report hashes are in
[`artifacts.tsv`](artifacts.tsv). The actual oracle and audit runtime was
Eclipse Temurin JDK 21.0.13. #2308's earlier closeout used Temurin 21.0.8;
this newer runtime remains Java 21, but the hosted Acceptance VI gate is still
unverified.

Reproduce the full matrix with the existing source and runner:

```sh
export TMPDIR="$MANAGED_SCRATCH_PATH" TMP="$TMPDIR" TEMP="$TMPDIR"
export SCALA39_ROOT="$PINNED_SCALA_3_9_0_CHECKOUT"
export JAVA_HOME="$JDK_21_HOME" PATH="$JAVA_HOME/bin:$PATH"
export SCALA39_JDK_RELEASE=21
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-minimal-method-cohort-3.9.0.tsv \
  --verify-determinism "$TMPDIR/main.tsv"
tools/typer-classpath-corpus-audit/run --method-kind ordinary \
  --method-manifest docs/typer-stdlib-holdout-acceptance-ii-3.9.0.tsv \
  --verify-determinism "$TMPDIR/analysis.tsv"
```

Repeat both commands for the independent second audit. The runner checks the
Scala source pin and uses the existing pinned Scala 3.9.0 artifact set. For the
original-source oracle, use the same `SCALA39_CLASSPATH` and run:

```sh
java -cp "$SCALA39_CLASSPATH" dotty.tools.dotc.Main \
  -classpath "$SCALA39_CLASSPATH" -Ystop-after:typer -Xprint:typer \
  -color:never -d "$TMPDIR/predef-oracle" \
  "$SCALA39_ROOT/library/src/scala/Predef.scala"
```

No Typer semantic code, cohort manifests, CI workflow, or new audit harness was
added. The only code diff adds diagnostic detail to the existing ordinary
method audit test output so the preexisting root and accepted-root evidence
includes canonical identities and signatures.

The final combined 50-root audit also ran the existing
`pinned_scala39_ordinary_member_method_audit` test twice in fresh sessions and
reconfirmed 50/50 attempted, 38 typed, 12 blocked. `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`, and
`python3 tools/check-frontend-boundaries.py` passed. `git diff --check` is part
of the final handoff validation. No full-corpus CI job was run.
