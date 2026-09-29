# TASTy companion-link audit (Scala 3.9.0)

The companion audit enters the checked-in Scala 3.9.0 library and compiler
TASTy fixtures into one store per corpus. It carries `TastySession` between
units, repeats each corpus in reverse path order, and compares the resulting
owner-qualified class/object pairs.

Run it with:

```sh
cargo test -p dotty-tasty-unpickler --release --test companion_corpus -- --ignored --nocapture
```

The fixture manifests pin both corpora to Scala revision
`777528f19a58e794c9954a42f433373472ec57f8`. The compiler corpus contains
TASTy format 28.8.0 because the nonbootstrapped compiler artifact was built by
Scala 3.8.4; the audit parses both corpora as compatible with format 28.9.0.

## Results

| Corpus | Class/trait identities | Object identities | Reciprocal pairs | Trait pairs | Nested pairs | One-sided identities |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Scala library | 1,942 | 944 | 575 | 165 | 274 | 1,736 |
| Scala compiler | 2,327 | 2,170 | 1,191 | 50 | 743 | 2,115 |
| **Total** | **4,269** | **3,114** | **1,766** | **215** | **1,017** | **3,851** |

Both forward and reverse entry orders produced the same counts and pair sets.
The symmetric difference between pair sets was zero for each corpus. The
audit found zero ambiguous candidates, conflicting links, module-class
endpoints, or failed units. Case-class, enum, and given-generated identities
are included where present in these compiler corpora; the semantic projection
reports their common `Class`/`Trait`/`Object` identity categories.

One-sided identities count every class/trait or object that has no unique
same-named counterpart in its exact owner scope. They are expected for
declarations without a companion.
