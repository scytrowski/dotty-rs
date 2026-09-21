//! Semantic annotations, shared by `Type::Annotated` and `Symbol::annotations`.
//!
//! See `docs/dotty-core-design.md`, `[MAJOR 3]`: real Dotty stores
//! annotations in two places — a list directly on the symbol denotation, and
//! a single annotation on an annotated type — both of which resolve through
//! one [`Annotation`]/[`AnnotationArena`] here.

use crate::ast::Typed;
use crate::ids::{AnnotationId, SymbolId, TreeId, TypeId, checked_index};
use crate::names::TermName;
use crate::store::SemanticStore;
use crate::symbols::SymbolKind;
use crate::types::constant::Constant;
use crate::types::ty::Type;

/// One semantic annotation, e.g. `@deprecated` on a symbol or `@unchecked`
/// on a type.
///
/// `ty` is the annotation's type, which for a parameterised annotation is its
/// whole applied type. `arguments` are its term arguments, if the producer
/// reconstructed them. `tree` is an optional typed tree for later fidelity;
/// `None` means "no reconstructed typed tree is attached", not "the
/// annotation had no payload": [`AnnotationArguments`] carries that
/// distinction.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub ty: TypeId,
    pub arguments: AnnotationArguments,
    pub tree: Option<TreeId<Typed>>,
}

impl Annotation {
    /// An annotation whose term arguments the producer did not reconstruct
    /// ([`AnnotationArguments::Unavailable`]). This is what an annotation known
    /// only by its class `TypeId` is: it is never read as "no arguments".
    pub const fn new(ty: TypeId, tree: Option<TreeId<Typed>>) -> Self {
        Self {
            ty,
            arguments: AnnotationArguments::Unavailable,
            tree,
        }
    }

    /// A compact annotation: its type is the whole annotation, so it is known
    /// to have no term arguments and needs no tree. Type arguments stay in `ty`.
    pub const fn compact(ty: TypeId) -> Self {
        Self {
            ty,
            arguments: AnnotationArguments::Known(Vec::new()),
            tree: None,
        }
    }

    /// An annotation with exactly the term `arguments` given, in order, and
    /// no typed tree.
    pub fn with_arguments(ty: TypeId, arguments: Vec<AnnotationArgument>) -> Self {
        Self {
            ty,
            arguments: AnnotationArguments::Known(arguments),
            tree: None,
        }
    }
}

/// The term arguments of an [`Annotation`].
///
/// `Unavailable` and `Known(vec![])` are different facts: the first says the
/// producer did not look at the arguments, the second that there are none.
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationArguments {
    /// Not reconstructed by the producer.
    Unavailable,
    /// Every term argument, in the order written; type arguments are part of
    /// the annotation's type, not of this list.
    Known(Vec<AnnotationArgument>),
}

/// One term argument of an annotation.
#[derive(Clone, Debug, PartialEq)]
pub struct AnnotationArgument {
    /// The parameter name of a named argument (`name = value`), `None` for a
    /// positional one.
    pub name: Option<TermName>,
    pub value: AnnotationValue,
}

/// The value of an annotation argument: a closed, format-agnostic set that
/// grows only when a real form justifies a variant.
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationValue {
    /// A literal, including a class literal (`Constant::Class`).
    Constant(Constant),
}

impl SemanticStore {
    /// The class an annotation denotes by its type: the symbol of a `TypeRef`,
    /// or of the constructor of an `Applied` type. Any other type is not an
    /// annotation class.
    pub fn annotation_class(&self, annotation: &Annotation) -> Option<SymbolId> {
        if !self.types.is_filled(annotation.ty) {
            return None;
        }
        let mut ty = annotation.ty;
        if let Type::Applied { tycon, .. } = self.types.get(ty) {
            if !self.types.is_filled(*tycon) {
                return None;
            }
            ty = *tycon;
        }
        // A name-designated `TypeRef` has no class symbol: no text is compared.
        match self.types.get(ty) {
            ty @ Type::TypeRef { .. } => ty.reference_symbol(),
            _ => None,
        }
    }

    /// The names from the top-level package down to the class `symbol`, when
    /// every owner is a package and the chain ends at the root package (which
    /// has no name and is not part of the path). `None` for anything else, for
    /// example a class nested in an object: `p.O.C` is not `p.o.C`.
    pub fn class_path(&self, symbol: SymbolId) -> Option<Vec<&str>> {
        let mut path = vec![self.names.resolve(self.symbols.get(symbol).name.text())];
        let mut owner = self.symbols.get(symbol).owner;
        while let Some(package) = owner {
            let package = self.symbols.get(package);
            if package.kind != SymbolKind::Package {
                return None;
            }
            owner = package.owner;
            // The root package is the unnamed package with no owner.
            if owner.is_some() || !self.names.resolve(package.name.text()).is_empty() {
                path.push(self.names.resolve(package.name.text()));
            }
        }
        path.reverse();
        Some(path)
    }

    /// Whether `ty` carries an annotation whose class is exactly `path`
    /// (package segments, then the class name), following the chain of
    /// [`Type::Annotated`] wrappers at the outside of `ty`, as Dotty's
    /// `hasAnnotation` on a parameter type does. Nothing else is searched: an
    /// annotation on a type argument, or under any other type form, does not
    /// count. A wrapper around a binder that is still being decoded ends the
    /// chain.
    pub fn has_annotation(&self, ty: TypeId, path: &[&str]) -> bool {
        let mut current = ty;
        while self.types.is_filled(current) {
            let Type::Annotated {
                underlying,
                annotation,
            } = self.types.get(current)
            else {
                return false;
            };
            let annotation = self.annotations.get(*annotation);
            let class = self.annotation_class(annotation);
            if class.is_some_and(|class| self.class_path(class).is_some_and(|found| found == path))
            {
                return true;
            }
            current = *underlying;
        }
        false
    }
}

/// Allocates and looks up [`Annotation`] values.
#[derive(Debug, Default)]
pub struct AnnotationArena {
    annotations: Vec<Annotation>,
}

impl AnnotationArena {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn alloc(&mut self, annotation: Annotation) -> AnnotationId {
        let id = AnnotationId::new(checked_index(self.annotations.len()));
        self.annotations.push(annotation);
        id
    }

    /// The number of annotations allocated so far.
    pub(crate) fn len(&self) -> usize {
        self.annotations.len()
    }

    /// Drops every annotation allocated after the arena held `len`.
    pub(crate) fn truncate(&mut self, len: usize) {
        self.annotations.truncate(len);
    }

    /// Panics if `id` was not allocated by this arena — see
    /// `docs/dotty-core-design.md`, "Error handling policy."
    pub fn get(&self, id: AnnotationId) -> &Annotation {
        &self.annotations[id.index() as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_and_resolves_an_annotation_without_a_tree() {
        let mut arena = AnnotationArena::new();
        let ty = TypeId::new(1);

        let id = arena.alloc(Annotation::new(ty, None));

        assert_eq!(arena.get(id).ty, ty);
        assert_eq!(arena.get(id).tree, None);
    }

    #[test]
    fn unavailable_arguments_are_not_known_to_be_empty() {
        let ty = TypeId::new(1);

        assert_eq!(
            Annotation::new(ty, None).arguments,
            AnnotationArguments::Unavailable
        );
        assert_eq!(
            Annotation::compact(ty).arguments,
            AnnotationArguments::Known(Vec::new())
        );
        assert_ne!(Annotation::new(ty, None), Annotation::compact(ty));
        assert_ne!(
            Annotation::new(ty, None),
            Annotation::with_arguments(ty, Vec::new())
        );
    }

    #[test]
    fn arguments_keep_their_order_and_names() {
        let mut arena = AnnotationArena::new();
        let arguments = vec![
            AnnotationArgument {
                name: None,
                value: AnnotationValue::Constant(Constant::Int(1)),
            },
            AnnotationArgument {
                name: Some(TermName::new(crate::ids::NameId::new(7))),
                value: AnnotationValue::Constant(Constant::Int(2)),
            },
            AnnotationArgument {
                name: Some(TermName::new(crate::ids::NameId::new(7))),
                value: AnnotationValue::Constant(Constant::Int(3)),
            },
        ];

        let id = arena.alloc(Annotation::with_arguments(
            TypeId::new(1),
            arguments.clone(),
        ));

        // Duplicate names are kept as written, not merged.
        assert_eq!(
            arena.get(id).arguments,
            AnnotationArguments::Known(arguments)
        );
        assert_eq!(arena.get(id).tree, None);
    }

    #[test]
    fn distinct_allocations_get_distinct_ids() {
        let mut arena = AnnotationArena::new();
        let first = arena.alloc(Annotation::new(TypeId::new(1), None));
        let second = arena.alloc(Annotation::new(TypeId::new(2), None));

        assert_ne!(first, second);
    }

    use crate::names::{Name, Namespace};
    use crate::packages::Packages;
    use crate::symbols::{Symbol, SymbolFlags, SymbolInfo, SymbolLinks, SymbolOrigin, Visibility};

    const ERASED: [&str; 4] = ["scala", "annotation", "internal", "ErasedParam"];

    struct World {
        store: SemanticStore,
        packages: Packages,
        no_prefix: TypeId,
    }

    impl World {
        fn new() -> Self {
            let mut store = SemanticStore::new();
            let no_prefix = store.types.alloc(Type::NoPrefix);
            Self {
                store,
                packages: Packages::new(),
                no_prefix,
            }
        }

        /// A class `name` owned by the package `path`, or by `owner` if given.
        fn class(&mut self, path: &[&str], name: &str, owner: Option<SymbolId>) -> SymbolId {
            let owner = owner.unwrap_or_else(|| {
                self.packages
                    .enter(&mut self.store, SymbolOrigin::Synthetic, path)
                    .pop()
                    .unwrap()
                    .symbol
            });
            let name = Name::new(self.store.names.intern(name), Namespace::Type);
            self.store.symbols.alloc(Symbol {
                name,
                owner: Some(owner),
                kind: SymbolKind::Class,
                flags: SymbolFlags::EMPTY,
                visibility: Visibility::Public,
                info: SymbolInfo::Missing,
                origin: SymbolOrigin::Synthetic,
                annotations: Vec::new(),
                position: None,
                links: SymbolLinks::default(),
            })
        }

        fn type_ref(&mut self, symbol: SymbolId) -> TypeId {
            self.store
                .types
                .alloc(Type::type_ref(self.no_prefix, symbol))
        }

        fn annotate(&mut self, underlying: TypeId, class: TypeId) -> TypeId {
            let annotation = self.store.annotations.alloc(Annotation::new(class, None));
            self.store.types.alloc(Type::Annotated {
                underlying,
                annotation,
            })
        }

        fn erased_param(&mut self) -> TypeId {
            let class = self.class(&["scala", "annotation", "internal"], "ErasedParam", None);
            self.type_ref(class)
        }
    }

    #[test]
    fn the_class_path_is_the_packages_then_the_class() {
        let mut world = World::new();
        let class = world.class(&["scala", "annotation", "internal"], "ErasedParam", None);

        assert_eq!(world.store.class_path(class), Some(ERASED.to_vec()));
    }

    #[test]
    fn a_class_nested_in_an_object_has_no_class_path() {
        let mut world = World::new();
        let object = world.class(&["scala", "annotation"], "internal", None);
        world.store.symbols.get_mut(object).kind = SymbolKind::Object;
        let nested = world.class(&[], "ErasedParam", Some(object));

        assert_eq!(world.store.class_path(nested), None);
    }

    #[test]
    fn a_class_in_the_unnamed_package_has_just_its_name() {
        let mut world = World::new();
        let class = world.class(&[], "Top", None);

        assert_eq!(world.store.class_path(class), Some(vec!["Top"]));
    }

    #[test]
    fn the_exact_class_is_found_on_an_annotated_type_and_its_wrapper_is_kept() {
        let mut world = World::new();
        let int = world.class(&["scala"], "Int", None);
        let int = world.type_ref(int);
        let erased = world.erased_param();
        let annotated = world.annotate(int, erased);

        assert!(world.store.has_annotation(annotated, &ERASED));
        assert!(matches!(
            world.store.types.get(annotated),
            Type::Annotated { .. }
        ));
    }

    #[test]
    fn a_class_with_the_same_name_in_another_package_is_not_it() {
        let mut world = World::new();
        let int = world.class(&["scala"], "Int", None);
        let int = world.type_ref(int);
        let other = world.class(&["scala", "annotation"], "ErasedParam", None);
        let other = world.type_ref(other);
        let annotated = world.annotate(int, other);

        assert!(!world.store.has_annotation(annotated, &ERASED));
    }

    #[test]
    fn the_annotation_is_found_anywhere_in_the_outer_chain() {
        let mut world = World::new();
        let int = world.class(&["scala"], "Int", None);
        let int = world.type_ref(int);
        let erased = world.erased_param();
        let other = world.class(&["p"], "Other", None);
        let other = world.type_ref(other);

        let erased_first = world.annotate(int, erased);
        let erased_first = world.annotate(erased_first, other);
        let erased_last = world.annotate(int, other);
        let erased_last = world.annotate(erased_last, erased);
        let without = world.annotate(int, other);

        assert!(world.store.has_annotation(erased_first, &ERASED));
        assert!(world.store.has_annotation(erased_last, &ERASED));
        assert!(!world.store.has_annotation(without, &ERASED));
        assert!(!world.store.has_annotation(int, &ERASED));
    }

    #[test]
    fn an_annotation_inside_a_type_argument_does_not_count() {
        let mut world = World::new();
        let int = world.class(&["scala"], "Int", None);
        let int = world.type_ref(int);
        let list = world.class(&["scala"], "List", None);
        let list = world.type_ref(list);
        let erased = world.erased_param();
        let annotated_argument = world.annotate(int, erased);
        let applied = world.store.types.alloc(Type::Applied {
            tycon: list,
            args: vec![annotated_argument],
        });

        assert!(!world.store.has_annotation(applied, &ERASED));
        assert!(world.store.has_annotation(annotated_argument, &ERASED));
    }

    #[test]
    fn an_applied_annotation_type_is_identified_by_its_constructor() {
        let mut world = World::new();
        let int = world.class(&["scala"], "Int", None);
        let int = world.type_ref(int);
        let erased = world.erased_param();
        let applied = world.store.types.alloc(Type::Applied {
            tycon: erased,
            args: vec![int],
        });
        let annotated = world.annotate(int, applied);

        assert!(world.store.has_annotation(annotated, &ERASED));
    }

    #[test]
    fn a_wrapper_around_a_binder_still_being_decoded_ends_the_chain() {
        let mut world = World::new();
        let reserved = world.store.types.reserve();
        let other = world.class(&["p"], "Other", None);
        let other = world.type_ref(other);
        let annotated = world.annotate(reserved.id(), other);

        // The unfilled slot is never read.
        assert!(!world.store.has_annotation(annotated, &ERASED));
        assert!(!world.store.has_annotation(reserved.id(), &ERASED));
    }
}
