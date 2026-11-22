## Enter an in-memory package

This synthetic container contains one empty package. It exercises declaration
entry and package identity without repository fixtures. It has no class or
method bodies and does not prove Scala interoperability.

```rust
use dotty_core::{Definitions, SemanticStore, SymbolKind};
use dotty_tasty::tasty::{
    EncodedSection, Header, NameTableBuilder, RawName, TastyFile, TastyFileBuilder,
    Writer, PACKAGE_TAG, TERMREFPKG_TAG,
};
use dotty_tasty_unpickler::tasty_unpickler::TastyUnpickler;

let mut names = NameTableBuilder::new();
let asts = names.intern(RawName::Utf8("ASTs".into()))?;
let package_name = names.intern(RawName::Utf8("example".into()))?;
let mut path = Writer::new();
path.write_u8(TERMREFPKG_TAG);
path.write_nat(package_name)?;
let mut ast = Writer::new();
ast.write_u8(PACKAGE_TAG);
ast.write_length_prefixed_bytes(path.as_slice())?;
let bytes = TastyFileBuilder::new(Header {
    major_version: 28, minor_version: 9, experimental_version: 0,
    tooling_version: "unpickler example".into(), uuid: [0; 16],
}, names.finish()?)
.with_section(EncodedSection::raw(asts, ast.into_inner()))
.encode_validated()?;
let file = TastyFile::parse_and_validate_scala_3_9(&bytes)?;
let mut store = SemanticStore::new();
let definitions = Definitions::bootstrap(&mut store);
let package = {
    let mut unpickler = TastyUnpickler::new(&file, &mut store, definitions);
    let index = unpickler.enter_symbols()?;
    index.symbol_at(0).ok_or("expected package at AST-section byte zero")?
};
assert_eq!(store.symbols.get(package).kind, SymbolKind::Package);
assert_eq!(store.names.resolve(store.symbols.get(package).name.text()), "example");
# Ok::<(), Box<dyn std::error::Error>>(())
```

For declarations, enter symbols before requesting type or signature completion.
Keep the original bytes alive while the decoded file is borrowed. Bootstrap
builtins once in the store; reuse the same package registry or
[`tasty_unpickler::TastySession`] across related units. Independent per-file
package registries can create different symbols for the same package spelling.

Entry and completion have separate atomic transactions. Their documented
rollback contracts preserve previous successful work; interned names and
registered origin identities can survive an unsuccessful entry attempt.
Unsupported shapes and unresolved external dependencies remain typed errors
or explicit pending states. Structural decoding does not imply semantic
completion, and semantic type projection is not source expression typing.
