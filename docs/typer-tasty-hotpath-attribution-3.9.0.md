# Typer TASTy hot-path attribution (Scala 3.9.0)

## Scope and method

This report follows issue #1569. It uses the bounded four-file manifest in
`typer-hot-path-profile-manifest-3.9.0.tsv`, pinned Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`, Scala 3.9.0 artifacts, and JDK
21. The audit selected six method observations and three match probes and ran
three serial warm samples on fresh main (`c31bba0`) and three after adding the
opt-in TASTy phase observer. Cargo/test concurrency was one.

The observer reports TASTy parse, AST address indexing, definition selection,
parent resolution, constructor/accessor extraction, and member projection.
Parent alias and bare-reference callbacks and constructor/member bare-reference
callbacks include calls, resolved/unresolved outcomes, and callback time.
Each observed resource records its binary identity, TASTy version, payload size
bucket, and decoder outcome. Existing load profiles retain module completion,
resource lookup, decode, and rollback measurements. Profiling remains disabled
unless classload profile collection is requested.

## Results

| Measure | Fresh main | Detailed observer | Difference |
|---|---:|---:|---:|
| Mean measured Cargo wall time | 19,443 ms | 19,237 ms | -206 ms (-1.1%) |
| Mean child CPU user time | 17.63 s | 17.41 s | -0.22 s |
| Selected method observations | 6 | 6 | same |
| Selected match probes | 3 | 3 | same |

The wall and CPU differences are within run-to-run noise; they do not indicate
a speedup. One earlier instrumented repeat reached 28.1 seconds while another
worktree was compiling concurrently, so that run was excluded from the
three-sample comparison. The final measured process tree peaked at about 121
MB sampled RSS.

The script's exact identity/outcome comparison passed for method observations,
match-probe identities, and match outcomes. All selected TASTy resources used
version 28.9.0 and decoded successfully. Their observed payloads fell into the
4–64 KiB and 64 KiB–1 MiB buckets. The per-run resource rows and phase counters
are emitted as `tasty-resources.tsv`, `tasty-phases.tsv`, and
`match-tasty-phases.tsv` by the profile tool.

Across the three match-probe runs, the classloader recorded 123 module
completion attempts per run's aggregate audit set, with roughly 26.0 seconds
of delegated module-completion time per run. The represented match classload
profiles contained 15 TASTy decode calls per run (about 2.1–2.2 seconds of
inclusive TASTy decode time per file profile, with profiles allowed to overlap).
Do not sum those inclusive values as disjoint CPU time. In this cohort, parent
resolution did not invoke alias or bare callbacks. Member projection invoked
three bare callbacks per run in the match profiles; all three were unresolved.
Those are callback outcomes, not evidence that the same classpath probe was
repeated unnecessarily. Existing aggregate classpath counters report resource
lookup hits and misses, but do not identify individual `find_class` probes by
TASTy callback.

Across the three measured repetitions, the source profiles recorded 24.16 s
of member projection, 84 ms of AST indexing, 51 ms of parent resolution, and
10 ms of parsing. The match profiles recorded 22.77 s of member projection,
82 ms of indexing, 48 ms of parent resolution, and 9 ms of parsing. These
profile timings are inclusive across classload observations and may overlap;
they are attribution totals, not disjoint process CPU time. The match resolver
recorded 123 module-completion calls and 25.995 s delegated time across all
three repetitions. Match member projection invoked nine unresolved bare-name
callbacks across the three files and three repetitions; parent alias and bare
callbacks were not invoked.

The detailed observer did not reveal a safely removable repeated operation.
The dominant reported phase remains member projection, but the current data
does not prove that any part of it is redundant. No optimization is made on
this evidence. Follow-up optimization should wait for a cohort with repeatable
same-key probe attribution inside one decode, or a focused trace showing
duplicate AST traversal that preserves all decoder outcomes when removed.

## Regression coverage

The classloader suite covers successful Scala 3.9 TASTy decoding, malformed
input, unresolved parent rollback and retry, and successful module completion.
The focused failed-decode profiling test asserts that parse time is captured
while later AST-index work remains zero, and that failed module completion is
reported as an error. The typer corpus audit compares exact selected identities
and outcomes with the pre-change baseline.

## Validation

- `cargo test -p dotty-classloader --lib --locked`: 443 passed, 4 ignored.
- `cargo test -p dotty-classloader --lib failed_module_tasty_decode_is_included_in_load_profile --locked`: passed after the final profile changes.
- `cargo check -p dotty-typer --test local_definition_audit --locked`: passed.
- `python3 -m py_compile tools/typer-classpath-corpus-audit/sequential_profile.py`: passed.
- Three-run serial Scala 3.9 cohort and exact comparison against fresh main: passed.

## Limits

The bounded manifest is a focused attribution cohort, not a full corpus
estimate. Wall/CPU timing differences are too small to establish instrumentation
overhead precisely. Resource lookup counters are aggregate classloader events;
the current callback layer does not expose per-candidate classpath probe counts.
The evidence supports a measurement closeout, not an optimization claim.
