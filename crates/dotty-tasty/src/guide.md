## Round-trip a structural container

This in-memory example has an empty ASTs section. It demonstrates container
validation and byte preservation, not a Scala compilation unit or semantic
pickler output.

```rust
use dotty_tasty::tasty::{
    EncodedSection, Header, NameTableBuilder, RawName, TastyFile, TastyFileBuilder,
};

let mut names = NameTableBuilder::new();
let asts = names.intern(RawName::Utf8("ASTs".into()))?;
let header = Header {
    major_version: 28, minor_version: 9, experimental_version: 0,
    tooling_version: "dotty-tasty example".into(), uuid: [0; 16],
};
let bytes = TastyFileBuilder::new(header, names.finish()?)
    .with_section(EncodedSection::raw(asts, Vec::new()))
    .encode_validated()?;
let file = TastyFile::parse_and_validate_scala_3_9(&bytes)?;
assert_eq!(file.encode()?, bytes);
assert!(TastyFile::parse(b"malformed").is_err());
# Ok::<(), Box<dyn std::error::Error>>(())
```

Borrowed readers and decoded views retain slices into the input bytes.
Builders and finished encoded fragments own their data. AST addresses are
relative to the ASTs section, name references use a separate zero-based table,
and symbolic construction labels belong to their originating builder.

Choose raw preservation, structured decoding, and validation according to the
required guarantee. Wire validity does not complete semantic symbols or prove
that Scala can consume a newly produced artifact. The supported reference
format is TASTy 28.9.0; compatible-version entry points express a separate
policy. Malformed data and configured depth or size limits return typed
errors rather than partially accepted semantic values.
