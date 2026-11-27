## Check an emitted class-file link against a TASTy header

This example constructs a synthetic pair with the same UUID and checks the
byte-level link. The empty ASTs section is only a structural container; the
pair does not establish Scala interoperability or semantic ABI agreement.

```rust
use dotty_classfile::classfile::{ClassFile, ClassFileBuilder, ClassFileVersion, Reader, TastyLinkId};
use dotty_tasty::tasty::{
    EncodedSection, Header, NameTableBuilder, RawName, TastyFileBuilder,
};
use dotty_jvm_tasty_contract::validate_classfile_tasty_link;

let uuid = [7; 16];
let mut names = NameTableBuilder::new();
let asts = names.intern(RawName::Utf8("ASTs".into()))?;
let tasty = TastyFileBuilder::new(Header {
    major_version: 28, minor_version: 9, experimental_version: 0,
    tooling_version: "contract example".into(), uuid,
}, names.finish()?)
.with_section(EncodedSection::raw(asts, Vec::new()))
.encode_validated()?;
let mut builder = ClassFileBuilder::new(
    "example/Answer", ClassFileVersion { major: 65, minor: 0 },
)?;
builder.tasty_link(TastyLinkId::from_bytes(uuid))?;
let bytes = builder.encode()?;
let class = ClassFile::decode(&mut Reader::new(&bytes))?;
validate_classfile_tasty_link(&class, &tasty)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

For semantic agreement, use [`validate_jvm_tasty_class`] and the whole-batch
validators with plans borrowing the same store, source roots, and typed arena.
A UUID match alone says nothing about owners, descriptors, parameter order,
member completeness, or body identity. Plan receipts retain input borrows;
owned artifact checks operate on the actual emitted bytes and exact paths.

Failures are [`JvmTastyContractError`] values. The checks do not mutate plans,
encode missing artifacts, write files, or broaden the audited overlap between
JVM and semantic TASTy producers. Same-slot IDs from another compilation
session cannot be authenticated, so the single-session requirement remains a
caller obligation.

Ordinary Int/Boolean locals and pattern bindings retain exact declaration and
symbol identities across both plans. Ordinary bindings must also agree on
mutability; pattern proofs include their original case and scope. A shared
Integer verifier category does not make an Int local a valid Boolean binding.
