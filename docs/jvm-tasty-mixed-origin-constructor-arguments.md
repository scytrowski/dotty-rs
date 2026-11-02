# Mixed-origin Pair constructor arguments audit (#2011)

This is an audit-only result. It records the Scala 3.9.0 representation and
the current Rust first rejection for a Pair constructor call with one existing
method formal and one nested `new Counter(seed)`. It changes no production
semantics and establishes no paired JVM/TASTy feature support.

## Frozen inputs and oracle

The audit starts from `origin/main` at
`7943e8d495815ceef2b7e4a53baee7f6fcac949b`. Scala is pinned to revision
`777528f19a58e794c9954a42f433373472ec57f8`, staged compiler
`3.9.0-bin-SNAPSHOT-git-777528f`, and Temurin JDK `21.0.13+9` with
`--release 21`. `oracle.json` retains the staged compiler JAR SHA-256 as an
informational receipt because sbt may package the same pinned sources with
different ZIP metadata. The `java` binary SHA-256 is
`9cad3f2620d3ef2ed10b26f922af7e773d54fdaa05f615d6d6df014f3fec4155`.
The compiler JAR receipt currently recorded is
`b56626390028bd06347e3549cf271a03ca7d2e7be491ceff630ccee94e439f4f`; the
reproduction comparison deliberately ignores only that field.
`oracle.json` records the exact producer and client source hashes, the six
producer artifact hashes for each ordering, classfile/TASTy UUID links, and
hashes for normalized snapshots. Normalization replaces only the dump's input
path and javap modification-date header; bytecode offsets, constant-pool
references, artifact checksums, and the rest of each dump remain compared.

The two producer inputs are separate frozen files under
[`tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments`](../tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments):

| Input | Method body | SHA-256 |
| --- | --- | --- |
| `wrap-and-create/PairFactory.scala` | `new Pair(existing, new Counter(seed))` | `9232b0a536470a69041f399a23f2b4a0e80a9dcfeb28575f17a84d1f146f8935` |
| `create-and-wrap/PairFactory.scala` | `new Pair(new Counter(seed), existing)` | `0c14002f9915683fe60371ac53ea36bec77b24b8a04781152fbae020449ac95c` |

Each order was compiled into its own empty output directory with the same
three declarations: `Counter`, `Pair`, and `PairFactory`. Each output contains
exactly `Counter`, `Pair`, and `PairFactory` `.class` files and their three
`.tasty` sidecars. All classfiles have major version 65, all sidecars are TASTy
28.9.0, and each classfile's `TASTY` attribute equals its corresponding
sidecar UUID. The binary files and `javap -v -p -c` / `-print-tasty` snapshots
are checked in as Scala oracle output only. Rebuild both variants and run
their separate Scala-only clients with:

```sh
export SCALA39_ROOT=/path/to/scala3-at-777528f19a58e794c9954a42f433373472ec57f8
export JAVA_HOME=/path/to/temurin-21.0.13
export PATH="$JAVA_HOME/bin:$PATH"
export TMPDIR="$PWD/target/issue-2011/tmp"
export DOTTY_ISSUE_2011_ORACLE_DIR="$PWD/target/issue-2011/oracle"
tests/fixtures/jvm_tasty_mixed_origin_constructor_arguments/rebuild-oracle.sh
```

The rebuild first runs `test_oracle_tools.py`, which verifies that paths and
dates normalize while bytecode evidence is preserved, and that manifest
comparison ignores only the informational compiler JAR digest.

The clients compile against only the matching Scala producer output and Scala
runtime. Each creates `Counter(7)`, invokes its one factory method with seed
`3`, repeats it with seed `-9`, checks that the original formal object reaches
the requested Pair field, checks that each nested allocation is fresh, and
mutates the formal and allocated Counters independently. Both clients pass.
They do not consume Rust-produced artifacts.

## Scala classfile and TASTy observations

Both methods have descriptor
`(Lexample/Counter;I)Lexample/Pair;`; `existing` is local 1 and `seed` is
local 2. The actual `javap` snapshots record constant-pool references and
offsets. For `wrapAndCreate`, the method reports `stack=6, locals=3`:

| Offset | Instruction | Resolved constant-pool entry |
| ---: | --- | --- |
| 0 | `new #18` | `example/Pair` |
| 3 | `dup` | — |
| 4 | `aload_1` | existing `Counter` formal |
| 5 | `new #20` | `example/Counter` |
| 8 | `dup` | — |
| 9 | `iload_2` | seed `Int` formal |
| 10 | `invokespecial #23` | `Counter.<init>(I)V` |
| 13 | `invokespecial #26` | `Pair.<init>(Counter, Counter)V` |
| 16 | `areturn` | — |

For `createAndWrap`, the method reports `stack=5, locals=3`: Pair is
allocated at offset 0, then Counter at offset 4, `iload_2` at 8,
`Counter.<init>(I)V` at 9, `aload_1` at 12, and
`Pair.<init>(Counter, Counter)V` at 13, followed by `areturn` at 16.
The two methods each perform one outer Pair allocation and one inner Counter
allocation. Evaluation of the constructor arguments follows their source
order, so the `aload_1` and nested allocation exchange order between variants.
The actual constant-pool entries are listed in each checked-in `javap`
snapshot; no offset or pool index is inferred from source text.

In both TASTy files, `PairFactory` contains a `DEFDEF` with ordered
`PARAM existing: Counter`, `PARAM seed: Int`, and result `Pair`. Its body is
an outer `APPLY(SELECTin(NEW(Pair)))`. In the first input the argument children
are `TERMREFdirect(existing)` followed by
`APPLY(SELECTin(NEW(Counter)), TERMREFdirect(seed))`. In the reversed input
those children are reversed. The selected constructor signatures are
`Pair.<init>(Counter, Counter): Pair` and `Counter.<init>(Int): Counter`.
These TASTy tree children preserve semantic source order; the JVM instruction
offsets independently show evaluation order.

## One-session Rust observations

`mixed_origin_pair_arguments_reach_jvm_and_report_the_proof_boundary` runs
each frozen input through the real Lexer → Parser → Namer → SourceTyper and
normal JVM source-compilation pipeline. The two orderings use independent
sessions and `SourceId(20110)` / `SourceId(20111)`. The following arena numbers
are receipts for these runs only; equal numbers across the two independently
created stores do not denote shared identities.

| Source item | `wrapAndCreate` session | `createAndWrap` session |
| --- | --- | --- |
| `Counter`, `Pair`, `PairFactory` class symbols | 40, 41, 42 | 40, 41, 42 |
| Factory method declaration / symbol | `TreeId(59)` / `SymbolId(60)` | `TreeId(59)` / `SymbolId(60)` |
| Formals `(Counter, Int)` | `(TreeId(45), SymbolId(61), TypeId(66))`, `(TreeId(47), SymbolId(62), Definitions.int)` | same run-local values |
| Counter and Pair constructors selected by the body | `SymbolId(44)`, `SymbolId(52)` | same run-local values |
| Outer Pair `Apply` source / typed root | `TreeId(58)` / `TreeId(22)` | same run-local values |
| Outer Pair `Select` and `New`, source / typed roots | `51/15`, `50/14` | same run-local values |
| Source argument roots / typed roots | `52,57` / `16,21` | `56,57` / `20,21` |
| Nested Counter `Apply`, `Select`, `New`, seed, source / typed roots | `57/21`, `55/19`, `54/18`, `56/20` | `56/20`, `54/18`, `53/17`, `55/19` |
| Source argument spans in file order | `437..445`, `447..464` | `437..454`, `456..464` |

The `Counter` formal and `Int` formal are distinct canonical method-owned
symbols. Their declarations have no typed declaration roots. The direct
`Counter` argument remains an `Ident` with a typed `TermRef` to `SymbolId(61)`;
the inner `Counter` argument remains an `Apply` whose seed `Ident` has a typed
`TermRef` to `SymbolId(62)`. The original `SourceTypedIndex` retains each
source-to-typed edge. The outer and inner constructor selections resolve to
the actual `Pair.<init>` and `Counter.<init>` symbols in the same source unit.

The normal JVM-only path emits the three classfiles for each input. The audit
asserts the factory method descriptor, `max_locals=3`, selected constructor
references, and bytecode order with exactly one Pair and one Counter
allocation. This path emits no TASTy sidecars. It is evidence about the
existing JVM-only source backend and does not establish paired output.

| Reachable Rust stage | First observed result |
| --- | --- |
| Lexer → Parser → Namer → SourceTyper | Passes for both inputs; original source and typed roots and identities are retained. |
| JVM-only source compilation and emission | Passes for both inputs; emits `Counter`, `Pair`, and `PairFactory` classfiles. |
| Cross-class constructor-call proof | Rejects both at `SemanticIdentity("both factory formals must retain the same canonical reference class")`. The pair proof requires equal canonical formal classes, while these formals have types `Counter` and `Int`. |
| Paired JVM/TASTy batch | First rejects in phase `cross-class constructor-call identity proof` with the same semantic identity error; its callback does not run. |
| Paired signature/body validation, pickler writer, TASTy output, and paired JVM artifact publication | Masked by the earlier cross-class proof error; none is reported as an observed later failure. |

The Scala oracle describes a valid language-level call shape. The Rust
first-error result describes only the current paired planner boundary; it is
not a claim that the Rust paired path accepts or emits this shape.

## Negative and adjacent-case matrix

| Variation | Audit result |
| --- | --- |
| Swap the two constructor argument positions | Measured as the separate valid `createAndWrap` input in Scala, Scala client, and the authentic Rust source session. |
| Rename classes/package/formals or reorder declarations | Not measured in this issue audit. |
| Pass `Int` where `Counter` is required; wrong Pair constructor arity | Not measured here. No Rust behavior or Scala diagnostic is claimed. |
| Same-named foreign `Counter`; missing or duplicate method formal; different Counter owner | Not measured here. |
| Test-only mutation of the source/typed target binding | Not measured. The confirmed earlier formal-class guard would need to be accounted for before interpreting a later identity result. |
| Third constructor argument; second nesting level; helper call, local value, or branch argument | Not measured here. These shapes are outside the frozen producer. |
| Using/default/generic/overloaded/secondary constructor alternatives | Not measured here. |
| Missing/corrupt TASTy sidecar or mismatched classfile/TASTy link | Not exercised for these inputs: the paired planner stops before it emits artifacts that could be passed to the link validator. |

The established neighboring evidence remains bounded to the earlier
Formal+Formal forwarding and NestedNew+NestedNew constructor-call audits in
[`jvm-tasty-dual-reference-forwarding.md`](jvm-tasty-dual-reference-forwarding.md)
and
[`jvm-tasty-multi-argument-constructor-calls.md`](jvm-tasty-multi-argument-constructor-calls.md).
Those results do not imply support for the mixed-origin pair audited here.

## Mixed method signature projection (#2013)

The semantic pickler accepts the original ordinary method signature
`wrapAndCreate(existing: Counter, seed: Int): Pair` when the method, `Counter`,
and `Pair` have canonical declarations in the same source and package, and the
two method formals retain their exact source-owned `SymbolId`s and declaration
order. Its completed plain `MethodType` must agree with those symbols: the
first formal is the exact planned `Counter` class reference, the second is
Scala `Int`, and the result is the exact planned `Pair` class reference. The
existing planned-class proof from #2012 authenticates the referenced classes;
the validator also checks their source definitions, package prefix, names,
ownership, and `ClassInfo`.

`ValidatedClassStatePlan::method_signature_skeleton` projects the two distinct
source `PARAM` declarations and the explicit `Counter`, `Int`, and `Pair` type
syntax. The skeleton retains each original method body root in its
`omitted_bodies` metadata and emits no RHS. It is a structural signature
projection, not a complete or Scala-readable sidecar. The original body
`new Pair(existing, new Counter(seed))` remains outside this increment and is
expected to fail body serialization until #2014. Reversing the constructor
argument expressions does not change method formal order. Source class order
and package/class/method/formal spellings may vary without replacing semantic
identity with name-based matching.

The bounded new form is exactly `(same-source class, Int) => same-package
class` in one plain value clause. Flipped `(Int, class)`, an added third formal,
foreign or unplanned classes, other result classes, multiple clauses,
erased/default/contextual/generic/overloaded signatures remain rejected. The
pre-existing one-class-reference method signature and same-typed dual-formal
signatures retain their earlier acceptance rules.

## Reproduction and validation

The focused Rust audit regression is:

```sh
export TMPDIR="$PWD/target/issue-2011/tmp"
export CARGO_TARGET_DIR="$PWD/target/issue-2011/cargo"
export CARGO_BUILD_JOBS=2
cargo test -p dotty --test jvm_source_compilation \
  mixed_origin_pair_arguments_reach_jvm_and_report_the_proof_boundary \
  --locked -- --nocapture
```

The Scala rebuild script above is strict about the Scala revision and compiles
each consumer separately. This increment changes only audit tests, frozen
oracle fixtures/manifests/scripts, and documentation. Existing fixture bytes
are not modified.
