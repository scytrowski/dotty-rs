# One source-authentic immutable constructor field

This contract records issue #2062's A2 projection checkpoint. The positive
control is the pinned Scala 3.9.0 source and binary already audited for #2059:

| Resource | SHA-256 |
| --- | --- |
| `Constructors.scala` | `ee054964fa24c1e2084350b257b0875d2c9c38038b73750b7eefb12b70ffcd63` |
| `CtorWithVal.tasty` | `53d335f857224d38559335eaf1efba92006a55ad9ed8e74d1f58a9e77f8a0192` |
| `CtorPlain.tasty` | `0d2f01cc5444bda30ae6a920162b3eb89fd2a5a7aefc84a1e22bef8a35120ddb` |

The source-authentic declaration is `class CtorWithVal(val x: Int)`. The
original TASTy index enters the class-header `PARAM` as a class-owned `Field`
and enters the constructor's separate `PARAM` as a constructor-owned
`Parameter`. Both have canonical `Int` type, and their `SymbolId`s differ.
The merged A1 audit records no separately encoded same-name TASTy getter;
this projection does not infer one or model JVM accessor behavior.

The private classloader projection first selects a narrow shape from structural
TASTy nodes and resolves every selected address through the entered
`TastySemanticIndex`. It completes the indexed field and constructor formal,
then checks their kinds, owners, shared source name, public visibility, empty
field flags, canonical `Int` type, and exactly one matching field in the
indexed class declaration scope. It does not allocate a same-spelled symbol.

The operation is transactional. A typed failure after field completion removes
the entered class from any surviving package scope, rolls back session and
semantic-store checkpoints, restores canonical builtin state, and leaves no
field cache entry. Retry uses the original indexed identity. On success, the
private field and its class scope are retained for this projection, while the
incomplete class is removed from the loader-visible package declarations and
is not inserted in the whole-class loading cache. The general private
`CtorWithVal` loader remains unsupported; the legacy classfile path is not
changed.

Supported shape is limited to one immutable class-header term field, one
same-named ordinary constructor formal, and no other class members. Mutable,
private constructor-only, multi-parameter, duplicate, wrong-owner, dangling,
malformed, or otherwise unsupported shapes fail before acceptance or roll back
without publishing a partial class. A3 must separately define any whole-class
eligibility and integration contract.
