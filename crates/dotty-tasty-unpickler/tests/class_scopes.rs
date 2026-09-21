//! Milestone 5d1: a completed `ClassInfo` publishes its declaration scope to
//! other units. Unit A is a real Scala 3.9.0 fixture; unit B is a synthetic
//! unit in the same package whose type nodes name A's members through name
//! references (`TYPEREF`, `TERMREF`, `TYPEREFin`), so B's own index never
//! holds a scope for any of A's classes.
use dotty_core::ids::SymbolId;
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolInfo, SymbolKind};
use dotty_core::types::Type;
use dotty_core::{Definitions, Packages};
use dotty_tasty::tasty::{
    Header, NameTable, PACKAGE_TAG, RawName, Section, SectionTable, TERMREFPKG_TAG, TYPEDEF_TAG,
    TastyFile,
};
use dotty_tasty_unpickler::tasty_unpickler::{TastyUnpickler, UnpickleError};

const CHILD: &[u8] = include_bytes!("fixtures/semantic/InfoChild.tasty");
const HOLDER: &[u8] = include_bytes!("fixtures/semantic/InfoHolder.tasty");
const BASE: &[u8] = include_bytes!("fixtures/semantic/InfoBase.tasty");
const DEP: &[u8] = include_bytes!("fixtures/semantic/InfoDep.tasty");
const PARENT: &[u8] = include_bytes!("fixtures/semantic/InfoParent.tasty");

const TYPEREF: u8 = 117;
const TERMREF: u8 = 115;
const TYPEREFIN: u8 = 175;
const IDENTTPT: u8 = 111;
const VALDEF: u8 = 129;

const NAMES: [&str; 20] = [
    "ASTs",
    "me",
    "cytrowski",
    "tastyfixtures",
    "semantic",
    "me.cytrowski",
    "me.cytrowski.tastyfixtures",
    "me.cytrowski.tastyfixtures.semantic",
    "InfoChild",
    "Member",
    "value",
    "InfoHolder",
    "Nested",
    "Inner",
    "deep",
    "v1",
    "v2",
    "v3",
    "v4",
    "v5",
];

fn n(text: &str) -> u32 {
    u32::try_from(NAMES.iter().position(|name| *name == text).unwrap()).unwrap()
}

fn nat(value: u32) -> Vec<u8> {
    let mut groups = vec![u8::try_from(value & 0x7f).unwrap() | 0x80];
    let mut rest = value >> 7;
    while rest > 0 {
        groups.push(u8::try_from(rest & 0x7f).unwrap());
        rest >>= 7;
    }
    groups.reverse();
    groups
}

fn node(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend(nat(u32::try_from(payload.len()).unwrap()));
    bytes.extend(payload);
    bytes
}

fn leaf(tag: u8, value: u32) -> Vec<u8> {
    let mut bytes = vec![tag];
    bytes.extend(nat(value));
    bytes
}

/// `TYPEREF`/`TERMREF name prefix`.
fn named(tag: u8, name: &str, prefix: &[u8]) -> Vec<u8> {
    [&[tag][..], &nat(n(name)), prefix].concat()
}

fn package() -> Vec<u8> {
    leaf(TERMREFPKG_TAG, n("me.cytrowski.tastyfixtures.semantic"))
}

fn info_child() -> Vec<u8> {
    named(TYPEREF, "InfoChild", &package())
}

fn info_holder() -> Vec<u8> {
    named(TERMREF, "InfoHolder", &package())
}

/// The five references, one per `VALDEF`, in this order.
const REFERENCES: [&str; 5] = [
    "a type member of a class",
    "a field of a class",
    "a class nested in an object",
    "a member of a nested object",
    "TYPEREFin",
];

fn unit_b() -> Vec<u8> {
    let types = [
        named(TYPEREF, "Member", &info_child()),
        named(TERMREF, "value", &info_child()),
        named(TYPEREF, "Nested", &info_holder()),
        named(TERMREF, "deep", &named(TERMREF, "Inner", &info_holder())),
        node(
            TYPEREFIN,
            &[nat(n("Member")), info_child(), info_child()].concat(),
        ),
    ];
    let definitions: Vec<u8> = types
        .iter()
        .enumerate()
        .flat_map(|(position, ty)| {
            let name = ["v1", "v2", "v3", "v4", "v5"][position];
            let tpt = [&[IDENTTPT][..], &nat(n(name)), ty].concat();
            node(VALDEF, &[nat(n(name)), tpt].concat())
        })
        .collect();
    let ast = node(PACKAGE_TAG, &[package(), definitions].concat());
    let names = NameTable::from_entries(
        NAMES
            .iter()
            .enumerate()
            .map(|(index, text)| match index {
                5 => RawName::Qualified {
                    prefix: 1,
                    selector: 2,
                },
                6 => RawName::Qualified {
                    prefix: 5,
                    selector: 3,
                },
                7 => RawName::Qualified {
                    prefix: 6,
                    selector: 4,
                },
                _ => RawName::Utf8((*text).to_owned()),
            })
            .collect(),
    )
    .unwrap();
    TastyFile::from_parts(
        Header {
            major_version: 28,
            minor_version: 9,
            experimental_version: 0,
            tooling_version: "Scala 3.9.0".to_owned(),
            uuid: [0; 16],
        },
        names,
        SectionTable::from_sections(vec![Section::new(0, &ast)]),
    )
    .unwrap()
    .encode()
    .unwrap()
}

/// The address of each reference of unit B: the type inside each `VALDEF`'s
/// `IDENTtpt`.
fn reference_addresses(file: &TastyFile<'_>) -> Vec<u32> {
    let index = file.ast_address_index().unwrap();
    let mut valdefs: Vec<u32> = index
        .iter_nodes()
        .filter(|node| node.tag == VALDEF)
        .map(|node| u32::try_from(node.offset).unwrap())
        .collect();
    valdefs.sort_unstable();
    let children = |at: u32| -> Vec<u32> {
        index
            .iter_tree_edges()
            .filter(|edge| edge.parent.offset == at as usize)
            .map(|edge| u32::try_from(edge.child.offset).unwrap())
            .collect()
    };
    valdefs
        .into_iter()
        .map(|valdef| children(children(valdef)[0])[0])
        .collect()
}

fn stub_classes(store: &mut SemanticStore, packages: &mut Packages, path: &[&str], names: &[&str]) {
    use dotty_core::names::{Name, Namespace};
    use dotty_core::symbols::SymbolOrigin;
    use dotty_core::{Symbol, SymbolFlags, SymbolLinks, Visibility};
    let package = packages
        .enter(store, SymbolOrigin::Synthetic, path)
        .pop()
        .unwrap();
    for class in names {
        let name = Name::new(store.names.intern(class), Namespace::Type);
        let symbol = store.symbols.alloc(Symbol {
            name,
            owner: Some(package.symbol),
            kind: SymbolKind::Class,
            flags: SymbolFlags::EMPTY,
            visibility: Visibility::Public,
            info: SymbolInfo::Missing,
            origin: SymbolOrigin::Synthetic,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        store.scopes.get_mut(package.scope).enter(name, symbol);
    }
}

struct Session {
    store: SemanticStore,
    definitions: Definitions,
    packages: Option<Packages>,
}

impl Session {
    fn new() -> Self {
        let mut store = SemanticStore::new();
        let definitions = Definitions::bootstrap(&mut store);
        let mut packages = Packages::new();
        stub_classes(
            &mut store,
            &mut packages,
            &["scala"],
            &["Int", "Any", "Nothing"],
        );
        stub_classes(&mut store, &mut packages, &["java", "lang"], &["Object"]);
        Self {
            store,
            definitions,
            packages: Some(packages),
        }
    }

    fn unit<R>(
        &mut self,
        bytes: &[u8],
        check: impl FnOnce(&mut TastyUnpickler<'_, '_, '_>, &TastyFile<'_>) -> R,
    ) -> R {
        let file = TastyFile::parse_scala_3_9(bytes).unwrap();
        let packages = self.packages.take().expect("packages");
        let mut unpickler =
            TastyUnpickler::with_packages(&file, &mut self.store, self.definitions, packages);
        unpickler.enter_symbols().expect("entering succeeds");
        let result = check(&mut unpickler, &file);
        let (_, packages) = unpickler.into_parts();
        self.packages = Some(packages);
        result
    }
}

/// The type definition of `kind` named `text` (module classes are `text$`).
fn class_at(
    unpickler: &TastyUnpickler<'_, '_, '_>,
    file: &TastyFile<'_>,
    text: &str,
    kind: SymbolKind,
) -> u32 {
    use dotty_tasty::tasty::{DefinitionBody, StructuredNode};
    let index = file.ast_address_index().unwrap();
    let mut found: Vec<u32> = index
        .iter_nodes()
        .filter(|node| node.tag == TYPEDEF_TAG)
        .map(|node| u32::try_from(node.offset).unwrap())
        .filter(|at| {
            let Ok(StructuredNode::TypeDef(DefinitionBody::TypeDef { name, .. })) =
                index.get(*at).unwrap().decode_structured()
            else {
                return false;
            };
            let rendered = match file.names().get(name) {
                Some(RawName::ObjectClass { underlying }) => file
                    .names()
                    .get_utf8(*underlying)
                    .map(|underlying| format!("{underlying}$")),
                _ => file.names().get_utf8(name).map(str::to_owned),
            };
            rendered.as_deref() == Some(text)
                && unpickler.symbol_state_at(*at).map(|state| state.0) == Some(kind)
        })
        .collect();
    found.sort_unstable();
    *found
        .first()
        .unwrap_or_else(|| panic!("no {kind:?} {text}"))
}

/// Enters the parents' units, so every parent of the classes below exists.
fn session_with_library_units() -> Session {
    let mut session = Session::new();
    for unit in [BASE, DEP, PARENT] {
        session.unit(unit, |_, _| ());
    }
    session
}

/// Unit A's classes, completed (or not), in a session; returns the symbols.
struct Owners {
    child: SymbolId,
    holder_class: SymbolId,
}

fn enter_owners(session: &mut Session, complete: bool) -> Owners {
    let child = session.unit(CHILD, |unpickler, file| {
        let at = class_at(unpickler, file, "InfoChild", SymbolKind::Class);
        if complete {
            unpickler.complete_symbol(at).unwrap();
        }
        unpickler.index().symbol_at(at).unwrap()
    });
    let holder_class = session.unit(HOLDER, |unpickler, file| {
        for (name, kind) in [
            ("InfoHolder$", SymbolKind::ModuleClass),
            ("Inner$", SymbolKind::ModuleClass),
            ("Nested", SymbolKind::Class),
        ] {
            if complete {
                let at = class_at(unpickler, file, name, kind);
                unpickler.complete_symbol(at).unwrap();
            }
        }
        let at = class_at(unpickler, file, "InfoHolder$", SymbolKind::ModuleClass);
        unpickler.index().symbol_at(at).unwrap()
    });
    Owners {
        child,
        holder_class,
    }
}

#[test]
fn without_class_completion_no_reference_decodes_and_nothing_is_completed_by_lookup() {
    let mut session = session_with_library_units();
    let owners = enter_owners(&mut session, false);
    let bytes = unit_b();
    let outcomes: Vec<_> = session.unit(&bytes, |unpickler, file| {
        reference_addresses(file)
            .into_iter()
            .map(|at| unpickler.unpickle_type(at))
            .collect()
    });
    for (outcome, label) in outcomes.iter().zip(REFERENCES) {
        // The owner class's scope is unknown, so the member is unresolved:
        // never a wrong answer.
        assert!(
            matches!(
                outcome,
                Err(UnpickleError::UnresolvedMember { .. }
                    | UnpickleError::UnsupportedResolutionSpace { .. }
                    | UnpickleError::UnsupportedResolutionPrefix { .. })
            ),
            "{label}: {outcome:?}"
        );
    }
    // Lookup is read-only: it completed nothing.
    for class in [owners.child, owners.holder_class] {
        assert_eq!(session.store.symbols.get(class).info, SymbolInfo::Missing);
    }
}

#[test]
fn a_completed_class_info_lets_another_unit_resolve_its_members() {
    let mut session = session_with_library_units();
    let owners = enter_owners(&mut session, true);
    let bytes = unit_b();
    let (outcomes, foreign_scopes) = session.unit(&bytes, |unpickler, file| {
        let outcomes: Vec<_> = reference_addresses(file)
            .into_iter()
            .map(|at| unpickler.unpickle_type(at))
            .collect();
        // The current unit's index holds no scope for either owner class:
        // ClassInfo, not index sharing, is what made them reachable.
        let foreign =
            [owners.child, owners.holder_class].map(|class| unpickler.index().scope_of(class));
        (outcomes, foreign)
    });
    assert_eq!(foreign_scopes, [None, None]);
    for (outcome, label) in outcomes.iter().zip(REFERENCES) {
        let ty = outcome
            .as_ref()
            .unwrap_or_else(|error| panic!("{label}: {error:?}"));
        assert!(
            matches!(
                session.store.types.get(*ty),
                Type::TypeRef { .. } | Type::TermRef { .. }
            ),
            "{label}"
        );
    }
}

#[test]
fn a_nested_class_is_published_by_its_own_class_info() {
    let mut session = session_with_library_units();
    enter_owners(&mut session, true);
    let bytes = unit_b();
    let nested = session.unit(&bytes, |unpickler, file| {
        unpickler
            .unpickle_type(reference_addresses(file)[2])
            .unwrap()
    });
    let symbol = session.store.types.get(nested).reference_symbol().unwrap();
    assert_eq!(session.store.symbols.get(symbol).kind, SymbolKind::Class);
    // The nested class has its own ClassInfo whose prefix is still the
    // canonical no_prefix.
    let SymbolInfo::Complete(info) = session.store.symbols.get(symbol).info else {
        panic!("nested class completed");
    };
    let Type::ClassInfo(info) = session.store.types.get(info) else {
        panic!("class info");
    };
    assert_eq!(info.prefix, session.definitions.no_prefix);
    assert_eq!(info.class, symbol);
    // Its owner is the object's module class, which lists it.
    let owner = session.store.symbols.get(symbol).owner.unwrap();
    assert_eq!(
        session.store.symbols.get(owner).kind,
        SymbolKind::ModuleClass
    );
}

#[test]
fn a_type_reference_in_another_unit_decodes_without_a_resolver() {
    // `TYPEREFin Member InfoChild InfoChild` is the survey's `REFin`: its
    // owner space is another unit's class.
    let mut session = session_with_library_units();
    enter_owners(&mut session, true);
    let bytes = unit_b();
    let ty = session.unit(&bytes, |unpickler, file| {
        unpickler.unpickle_type(reference_addresses(file)[4])
    });
    assert!(ty.is_ok(), "{ty:?}");
}
