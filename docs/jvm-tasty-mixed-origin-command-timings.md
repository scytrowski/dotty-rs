# Mixed-origin Scala consumer command timings

This target records sanitized external-command timings only when
`DOTTY_ORACLE_TIMINGS_FILE` is set. With the pinned Scala 3.9.0 environment
(`SCALA39_ROOT`, `JAVA_HOME`, and `COURSIER_MAVEN_ROOT`) available, run:

```sh
mkdir -p target
export DOTTY_REQUIRE_SCALA_TASTY_MIXED_ORIGIN_CONSTRUCTOR_CONSUMER=1
export DOTTY_ORACLE_TIMINGS_FILE="$PWD/target/mixed-origin-command-timings.jsonl"
: > "$DOTTY_ORACLE_TIMINGS_FILE"
cargo test -p dotty --test jvm_tasty_mixed_origin_constructor_scala_consumer --locked -- --nocapture
python3 - "$DOTTY_ORACLE_TIMINGS_FILE" <<'PY'
import json
import sys
from collections import defaultdict

groups = defaultdict(list)
with open(sys.argv[1], encoding="utf-8") as stream:
    for line in stream:
        event = json.loads(line)
        groups[event["category"]].append(event["elapsed_ms"])
for category, samples in sorted(groups.items()):
    print(f"{category}: count={len(samples)} total_ms={sum(samples):.3f}")
print("slowest operations:")
events = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
for event in sorted(events, key=lambda item: item["elapsed_ms"], reverse=True)[:10]:
    print(f"{event['category']} {event['operation']} {event['elapsed_ms']:.3f} ms {event['status']}")
PY
```

Rows follow the oracle timing JSONL fields (`event`, `category`, `operation`,
`elapsed_ms`, and `status`). They omit arguments, paths, source, environment
values, stdout, and stderr. All subprocesses in this integration target are
timed: Scala version/provenance checks, `sha256sum` checks, positive and
negative Scala compiles, Scala TASTy reader/printer operations, and positive
and negative JVM runs. The target has no `javap` call or `Command::status`
call; its commands capture output with `Command::output`. No timeout is used by
the existing target, and instrumentation does not add one.
