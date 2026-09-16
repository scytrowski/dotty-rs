# TASTy baseline oracle

This is the Scala 3.9.0 side of the baseline framework. It uses
`scala3-tasty-inspector` to load TASTy files and writes a deterministic JSON
projection containing the input path, tree kinds, and resolved symbol names.

The tool accepts individual `.tasty` files, directories, and JARs. A JAR is
materialized into a temporary directory before inspection. Use
`--classpath=` when the inspected TASTy files refer to class files from a
separate compiler output directory or dependency set.

```text
sbt 'run output.json path/to/classes --classpath=path/to/classes'
sbt 'run output.json path/to/library.jar'
```

To inspect only a deterministic subset, pass a newline-delimited selection
file. Blank lines and lines beginning with `#` are ignored; entries are
matched against the logical paths exposed by a directory or JAR input:

```text
sbt 'run output.json --select=semantic-selection.txt path/to/library.jar'
```

The output contains `schema_version = 2` and `scala_version = 3.9.0`. Each file
contains a sorted, duplicate-free `declarations` list with simplified
`TypeDef`, `DefDef`, and `ValDef` identifier names. Operator names are omitted
in this first version. The projection deliberately excludes absolute AST
offsets, types, overload-resolution details, and compiler-generated symbols.
It is a semantic-lite oracle, not a Rust semantic model.
