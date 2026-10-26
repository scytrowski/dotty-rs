# Classloader completion fanout policy, Scala 3.9.0

Status: **RESOLVED**. This report
covers the four frozen `scala/collection/immutable/List` requests and one
additional high-fanout request observed in the same three-file profile. The
historical `classloader-list-fanout-3.9.0.md` report remains unchanged.

## Pinned reproduction

The three-run focused profile was measured on dotty-rs source
`5a0503dc0ed4d0fdf7dd2d19add27dfdd3361acb`, Scala source
`777528f19a58e794c9954a42f433373472ec57f8` (3.9.0 artifacts), and Temurin
JDK `21.0.8+9`. Completed Scala identities are present in the pinned
`scala-library-3.9.0.jar`; completed `java.*` identities are in the pinned
`java.base.jmod`. The three selected source files produce 11 method
observations per run; all five requests above the legacy 16-completion bound
below were identical across all three serial runs.

| Source method | Request | Completed | Visited | Depth | Hierarchy edges | Member identity edges / requests / successes | Cache hits |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `ListMap.scala#tree=146` (`ListMap.addOne`) | `scala/collection/immutable/List` | 29 | 55 | 7 | 81 | 109 / 509 / 471 | 53 |
| `ListBuffer.scala#tree=1088` (`ListBuffer.removeAfter`) | `scala/collection/immutable/List` | 29 | 55 | 7 | 81 | 109 / 509 / 471 | 53 |
| `TrieMap.scala#tree=2721` (`LNode.inserted`) | `scala/collection/immutable/List` | 29 | 55 | 7 | 81 | 109 / 509 / 471 | 53 |
| `TrieMap.scala#tree=2780` (`LNode.removed`) | `scala/collection/immutable/List` | 29 | 55 | 7 | 81 | 109 / 509 / 471 | 53 |
| `TrieMap.scala#tree=4214` (`TrieMap.RDCSS_Complete`) | `scala/collection/mutable/AbstractMap` | 25 | 46 | 6 | 62 | 89 / 351 / 320 | 38 |

The three current profile outputs matched for each semantic TSV. SHA-256:

- root counters: `8b31e1c549aa06efc3a18542de50bb963e9547e47abc5f9f1c2b054625be4b8a`
- dependency edges: `c7fa4944eaf49086ae771ee49e3244a793a1be24bae6ad3483145f498c5deb57`
- visited/completed identities: `d3e8aa9f476036d5d1816713d7253e9a8a35814068f7e20f51d064c18e7a6126`

The refreshed compact oracle is split into
[`current-roots`](classloader-completion-fanout-3.9.0-current-roots.tsv),
[`current-edges`](classloader-completion-fanout-3.9.0-current-edges.tsv), and
[`current-classes`](classloader-completion-fanout-3.9.0-current-classes.tsv). Each
contains the same request identities across all three runs.

## Comparison with the frozen historical graph

The old evidence was measured at dotty-rs `616633b7e3e4b103dbaffd6a2e576bd92e82f41d`.
It reported 29 completions, 63 visited names, 81 hierarchy edges, 111 unique
member-identity edges, 435 member-identity requests, 300 successful member
references, and 53 completion-cache hits. The current main reproduces exactly
the same 29 completed binary names and exactly the same 81 directed hierarchy
edges for all four requests. Thus every full completion is still in the
pinned `List` parent/interface closure.

The complete visited/member-identity observations did change on current main:
55 names, 109 unique member-identity edges, 509 requests, and 471 successful
member references. This is not attributed to the fanout-policy work. Later
Scala type-projection changes changed member types referenced from the pinned
library; for example, the current graph records `scala/Any` and `scala/Nothing`
identity requests where the historical graph recorded several concrete
collection/factory type names. The completed class set and hierarchy graph
remain unchanged. The historical report is retained as historical evidence;
policy verification uses the current-main oracle linked above.

The same focused three-file run also surfaced
`TrieMap.scala#tree=4214::TrieMap.RDCSS_Complete`, requesting
`scala.collection.mutable.AbstractMap`. Its 25 completions are the exact
25-node hierarchy closure in the refreshed snapshot, with 62 hierarchy edges,
46 visited identities, and no completion attributable only to member-type
edges. This root is included in the current exception oracle because the
operational gate must classify every high-fanout request visible in this
bounded profile, not only the four List requests named in the issue.

The historical checker correctly rejects current run output because its
visited/member counters are frozen to the older source revision. A current-main
checker must compare against the refreshed oracle without rewriting that
historical evidence.

## Safety threat model

The replacement gate must distinguish:

1. a real, acyclic, possibly wide superclass/interface closure;
2. unnecessary eager completion caused by member/signature identity requests;
3. cycles, excessive inheritance depth, retries, rollback, or stale resource
   evidence;
4. pathological breadth caused by malformed or misprojected dependencies;
5. fake or caller-asserted “hierarchy” classifications that do not match the
   pinned classes and their checked-in graph.

A dependency reason alone is not proof. Exceptions must be tied to exact
request roots, completed identities, and an oracle-verified hierarchy graph.
Raw total completions remain visible.

## Candidate policies

- **Raise 16 to a larger scalar threshold:** rejected. It still confuses a
  genuine hierarchy with arbitrary member fanout and has no evidence-backed
  distinction.
- **Use dependency-reason labels alone to classify completions:** rejected.
  A caller-supplied or misprojected `hierarchy` label could hide unrelated
  completion work.
- **Allow a bounded, exact oracle-verified hierarchy exception, with a separate
  general safety budget:** selected for implementation. Only the five pinned
  requests from the focused three-file profile may exceed 16 without
  additional reviewed evidence. Their raw counts (29 for List, 25 for
  AbstractMap) and all edges remain visible. Unknown high-fanout requests fail
  closed.
  The checker must verify the complete class set and graph against the refreshed
  pinned snapshot, reject cycles and any completed identity outside the
  hierarchy closure, and enforce independent depth/identity/edge budgets.

The general budgets are fail-closed review limits, not Scala conformance
thresholds: 256 identities is the next power of two above four times the
largest historical visited set (63); 512 unique dependency edges is more than
2.6 times either measured high-fanout profile; hierarchy depth 256 reuses the
loader's existing `MAX_INHERITANCE_DEPTH`. Both observed load depth (which
omits cache-hit traversal) and graph depth are checked against that ceiling.
The focused List graph's observed depth is 7 while its full edge graph contains
an 8-edge path because shared ancestors were already completed. The existing
global p95 depth limit of 6 remains unchanged.

This decision does not change eager hierarchy semantics, class publication,
cache behavior, or the observed raw completions (29 for List and 25 for
AbstractMap).

## Implemented gate and validation

The corpus audit retains the historical p95 depth limit of 6 and reports the
unmodified maximum completion count. For requests above 16, it requires the
exact set of five pinned high-fanout roots and compares each request's counters,
visited/completed identities, and dependency edges with the current-main TSV
oracle. It then independently checks that every completion belongs to the
root's hierarchy closure, that the graph is acyclic, and that identity, edge,
and depth budgets hold. Missing evidence, duplicate roots, unknown roots,
member-only completions, altered edges, cycles, and budget overruns fail the
gate. The command-line output prints raw completions, proven hierarchy
completions, unexpected completions, observed depth, and identity count.

The gate tests cover all five pinned roots, shared ancestors, a wide valid
hierarchy, fake or altered edges, member-only completions, incomplete closures,
cycles, unknown roots, and each budget. Existing classloader rollback,
resource invalidation, and loader-session tests remain passing.

Validation completed:

- Three serial focused Scala 3.9.0 profiles passed `check_fanout_policy.py`;
  each reported five pinned roots, maximum raw completions 29, zero unexpected
  completions, and identical evidence across runs.
- Python audit suite: 102 tests passed.
- `cargo test -p dotty-classloader --lib --locked`: 442 passed, 4 ignored.
- Frontend dependency-boundary check, `cargo fmt --all -- --check`, and
  `git diff --check` passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`,
  `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked`, and
  `cargo test --workspace --all-targets --locked` passed.

## Reproduction

```sh
git worktree add --detach target/typer-list-fanout-source \
  5a0503dc0ed4d0fdf7dd2d19add27dfdd3361acb
(
cd target/typer-list-fanout-source
SCALA39_ROOT=/path/to/pinned/scala-3.9.0 \
JAVA_HOME=/path/to/temurin-21.0.8 \
SCALA39_JDK_RELEASE=21 \
COURSIER_MAVEN_ROOT=/path/to/pinned-maven-cache \
TMPDIR="$PWD/target/tmp" CARGO_TARGET_DIR="$PWD/target" \
CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 \
python3 tools/typer-classpath-corpus-audit/sequential_profile.py \
  --manifest docs/typer-list-fanout-manifest-3.9.0.tsv \
  --runs 3 --mode full --output-dir ../typer-list-fanout-current
)
python3 tools/typer-classpath-corpus-audit/check_fanout_policy.py \
  target/typer-list-fanout-current
git worktree remove --force target/typer-list-fanout-source
```

The detached worktree ensures the profile's recorded dotty-rs source SHA is the
frozen `5a0503d` revision required by the checker; the checker itself runs from
the current checkout so it can read the refreshed oracle. This reproducer does
not run the 1,236-file source corpus.
