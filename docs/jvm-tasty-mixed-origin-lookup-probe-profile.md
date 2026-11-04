# Mixed-origin lookup probe profile

Issue #2100 reduces repeated positive Scala lookups in `lookup_probes()` in
`tests/jvm_tasty_mixed_origin_constructor_scala_consumer.rs`. The six-member
inventory is counter get, counter set, pair left, pair right, pair constructor,
and factory wrap-and-create (using the fixture's selected factory method).

## Probe behavior

Each `lookup_probes()` invocation now copies the original Rust-produced class
files and TASTy sidecars, masks all six target classfile methods while retaining
the TASTy metadata, and compiles one Scala source with six separately named
definitions. The definitions preserve the original expressions. The test
checks that Scala emits each of the six corresponding `probe_*` methods with a
`Code` attribute, so every positive member remains independently observable.
The producer's input artifact hashes are checked before and after the lookup.

The six classfile-only controls remain independent. Each uses its own copied
fixture, masks only its target member, removes the TASTy marker and sidecar, and
asserts the first diagnostic: `[E008] Not Found Error` for counter get, counter
set, pair left, pair right, and factory wrap-and-create; `[E173] Reference
Error` for the pair constructor. No `javap` or runtime calls are part of these
probes.

## Measurements

All measurements ran the pinned Scala 3.9.0 consumer workflow with Scala source
revision `777528f19a58e794c9954a42f433373472ec57f8`, JDK `21.0.13+9.0.LTS`,
runner image Ubuntu 26.04, and the exact restored Scala build-cache key
`Linux-X64-scala39-targets-777528f19a58e794c9954a42f433373472ec57f8-jdk21.0.13-f59d197fbb3cf17975bc998b9fe65c368511db193bfb5e87d86b844bf139432b`.
The first baseline run populated that cache; its later runs and all candidate
runs restored it. Each target test calls `lookup_probes()` three times. The
totals below are per call, with three values and their order retained; target
test times cover all three calls and the rest of that test.

| Profile | Measured head | Workflow job | Cache | Target test | `lookup_probes()` totals (ms; 3 calls) | Calls per `lookup_probes()` |
| --- | --- | --- | --- | ---: | --- | ---: |
| Baseline 1 | `383e2d6029c4ff2b27f5e8f97ef90f05e5da172d` | [job 126843518807](https://github.com/scytrowski/dotty-rs/actions/runs/41906799498/job/126843518807) | initial miss, then saved | 224.73 s | 33,822.886; 33,349.216; 33,166.773 | 12 |
| Baseline 2 | same | [job 126853743947](https://github.com/scytrowski/dotty-rs/actions/runs/41906799498/job/126853743947) | hit | 224.09 s | 32,588.246; 33,002.546; 33,866.754 | 12 |
| Baseline 3 | same | [job 126881324808](https://github.com/scytrowski/dotty-rs/actions/runs/41906799498/job/126881324808) | hit | 232.74 s | 34,483.609; 34,814.806; 34,582.414 | 12 |
| Batched 1 | `de8050d7254877cbd425b7c904616e9099913b58` | [job 126866332399](https://github.com/scytrowski/dotty-rs/actions/runs/41913200648/job/126866332399) | hit | 192.07 s | 21,341.617; 21,662.603; 21,551.580 | 7 |
| Batched 2 | same | [job 126871861367](https://github.com/scytrowski/dotty-rs/actions/runs/41913200648/job/126871861367) | hit | 193.08 s | 21,392.773; 21,152.685; 21,931.646 | 7 |
| Batched 3 | same | [job 126876705706](https://github.com/scytrowski/dotty-rs/actions/runs/41913200648/job/126876705706) | hit | 187.50 s | 21,085.109; 21,078.469; 20,864.207 | 7 |

Across the nine calls in each profile, the mean `lookup_probes()` time fell
from 33,741.917 ms to 21,340.077 ms, a 36.8% reduction. Each invocation now
uses one positive `scalac` process for all six independently asserted members
plus the same six separate negative processes: 7 instead of 12 calls. A target
test therefore uses 21 lookup subprocesses instead of 36. Mean target-test
time fell from 227.19 s across the three baseline runs to 190.88 s across the
three batched runs.

The measured heads include temporary timing-only output around the probe
compiles; that instrumentation was removed from the final executable diff.
The final change retains the measured batching behavior and per-member and
negative-control assertions.
