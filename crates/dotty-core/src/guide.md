## Create and retain one semantic session

Bootstrap builtins once, then keep the store, definitions, package registry,
and phase arenas together. A numeric ID only identifies a slot in its owning
arena; an ID from another session can have the same slot and cannot generally
be detected as foreign. Rollback invalidates identities allocated after the
checkpoint, even if a later allocation reuses their numbers.

```rust
use dotty_core::{Definitions, Name, Namespace, Packages, SemanticStore, Type};

let mut store = SemanticStore::new();
let definitions = Definitions::bootstrap(&mut store);
let packages = Packages::new();
let spelling = store.names.intern("Answer");
let term = Name::new(spelling, Namespace::Term);
let ty = Name::new(spelling, Namespace::Type);
assert_ne!(term, ty);
assert_eq!(store.names.resolve(term.text()), "Answer");
assert!(matches!(store.types.get(definitions.int), Type::TypeRef { .. }));
assert!(packages.is_empty());
```

[`SemanticStore::checkpoint`] and [`SemanticStore::rollback_to`] undo new
semantic allocations and the store's recorded default-getter relations. They
do not generally restore arbitrary edits to pre-existing symbols and scopes.
A transaction that changes existing entries needs its own journal; naming and
typing maintain such phase-specific state. Syntax arenas and [`Packages`]
also have their own rollback mechanisms.

## Convert byte positions for an editor

Source spans use UTF-8 byte offsets. Editor columns can count UTF-8 bytes or
UTF-16 code units; they are not Unicode scalar or display-width columns.

```rust
use dotty_core::source::{DocumentText, PositionEncoding};

let text = DocumentText::new("ż😀\r\nx".into())?;
assert_eq!(text.position(6, PositionEncoding::Utf16), Some((0, 3)));
assert_eq!(text.offset(0, 3, PositionEncoding::Utf16), Some(6));
assert_eq!(text.position(7, PositionEncoding::Utf16), None); // inside CRLF
assert_eq!(text.offset(0, 2, PositionEncoding::Utf16), None); // inside emoji
# Ok::<(), Box<dyn std::error::Error>>(())
```

Unchecked arena getters can panic on invalid indexes. Checked queries such as
[`TypeArena::try_get`] reject absent or unfilled slots, but still cannot
establish the provenance of a same-slot foreign ID. Symbol ownership, scope
membership, type binders, and original source-to-typed bindings remain
separate contracts. Allocation and syntactic construction do not perform
Scala semantic checking.
