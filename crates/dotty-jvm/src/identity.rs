//! JVM names and class identity derived from canonical semantic symbols.

use std::collections::HashSet;

use dotty_classfile::access_flags::{ACC_FINAL, ACC_PUBLIC, ACC_SUPER, ClassAccessFlags};
use dotty_core::store::SemanticStore;
use dotty_core::symbols::{SymbolFlags, SymbolInfo, SymbolKind, Visibility};
use dotty_core::types::Type;

use crate::{
    InvalidJvmClassInfoKind, JvmClassIdentity, JvmCodegenError, JvmNameKind,
    UnsupportedJvmClassKind, UnsupportedJvmSymbolFlag,
};

pub(super) fn class_identity(
    store: &SemanticStore,
    class_id: dotty_core::SymbolId,
) -> Result<JvmClassIdentity, JvmCodegenError> {
    let class = store
        .symbols
        .contains(class_id)
        .then(|| store.symbols.get(class_id))
        .ok_or(JvmCodegenError::UnknownClass { class: class_id })?;
    if class.kind != SymbolKind::Class {
        return Err(JvmCodegenError::UnsupportedSymbolKind {
            symbol: class_id,
            expected: crate::SymbolRole::Class,
            found: class.kind,
        });
    }
    if class.visibility != Visibility::Public {
        return Err(JvmCodegenError::UnsupportedJvmVisibility {
            symbol: class_id,
            visibility: class.visibility,
        });
    }

    validate_class_flags(class_id, class.flags)?;
    validate_class_info(store, class_id, class.info)?;

    let class_name = store
        .names
        .try_resolve(class.name.text())
        .ok_or_else(|| invalid_name(class_id, JvmNameKind::Class, "<unresolved>"))?;
    if !is_simple_identifier(class_name) {
        return Err(invalid_name(
            class_id,
            JvmNameKind::Class,
            class_name.to_owned(),
        ));
    }

    let mut package_segments = Vec::new();
    let mut seen_owners = HashSet::new();
    let mut owner = class
        .owner
        .ok_or(JvmCodegenError::InvalidPackageOwnerChain {
            class: class_id,
            owner: class_id,
        })?;

    loop {
        if !seen_owners.insert(owner) {
            return Err(JvmCodegenError::InvalidPackageOwnerChain {
                class: class_id,
                owner,
            });
        }
        let package = store
            .symbols
            .contains(owner)
            .then(|| store.symbols.get(owner))
            .ok_or(JvmCodegenError::UnknownOwner {
                class: class_id,
                owner,
            })?;
        if package.kind != SymbolKind::Package {
            return Err(JvmCodegenError::UnsupportedOwnerKind {
                class: class_id,
                owner,
                found: package.kind,
            });
        }

        let segment = store
            .names
            .try_resolve(package.name.text())
            .ok_or_else(|| invalid_name(owner, JvmNameKind::PackageSegment, "<unresolved>"))?;
        match package.owner {
            None if segment.is_empty() => break,
            None => {
                return Err(JvmCodegenError::InvalidPackageOwnerChain {
                    class: class_id,
                    owner,
                });
            }
            Some(_) if segment.is_empty() => {
                return Err(JvmCodegenError::InvalidPackageOwnerChain {
                    class: class_id,
                    owner,
                });
            }
            Some(parent) => {
                if !is_simple_identifier(segment) {
                    return Err(invalid_name(
                        owner,
                        JvmNameKind::PackageSegment,
                        segment.to_owned(),
                    ));
                }
                package_segments.push(segment.to_owned());
                owner = parent;
            }
        }
    }

    package_segments.reverse();
    package_segments.push(class_name.to_owned());
    let internal_name = package_segments.join("/");
    Ok(JvmClassIdentity {
        symbol: class_id,
        internal_name,
        access_flags: ClassAccessFlags(
            ACC_PUBLIC
                | ACC_SUPER
                | if class.flags.contains(SymbolFlags::FINAL) {
                    ACC_FINAL
                } else {
                    0
                },
        ),
    })
}

fn validate_class_info(
    store: &SemanticStore,
    class: dotty_core::SymbolId,
    info: SymbolInfo,
) -> Result<(), JvmCodegenError> {
    let ty = match info {
        SymbolInfo::Missing | SymbolInfo::Deferred(_) => {
            return Err(JvmCodegenError::MissingSemanticInfo { symbol: class });
        }
        SymbolInfo::Error => return Err(JvmCodegenError::ErrorSemanticInfo { symbol: class }),
        SymbolInfo::Complete(ty) => ty,
    };
    let Some(semantic_type) = store.types.try_get(ty) else {
        return Err(invalid_class_info(
            class,
            Some(ty),
            InvalidJvmClassInfoKind::MissingOrUnfilled,
        ));
    };
    let Type::ClassInfo(class_info) = semantic_type else {
        return Err(invalid_class_info(
            class,
            Some(ty),
            match semantic_type {
                Type::Error(_) => InvalidJvmClassInfoKind::Error,
                _ => InvalidJvmClassInfoKind::NotClassInfo,
            },
        ));
    };
    if class_info.class != class {
        return Err(invalid_class_info(
            class,
            Some(ty),
            InvalidJvmClassInfoKind::WrongClassSymbol,
        ));
    }
    if !store.scopes.contains(class_info.declarations) {
        return Err(invalid_class_info(
            class,
            Some(ty),
            InvalidJvmClassInfoKind::MissingDeclarationScope,
        ));
    }
    if store.scopes.get(class_info.declarations).owner != Some(class) {
        return Err(invalid_class_info(
            class,
            Some(ty),
            InvalidJvmClassInfoKind::WrongDeclarationScopeOwner,
        ));
    }

    let mut declarations = store
        .scopes
        .get(class_info.declarations)
        .entered_symbols()
        .collect::<Vec<_>>();
    declarations.sort_by_key(|symbol| symbol.index());
    for declaration in declarations {
        let Some(symbol) = store
            .symbols
            .contains(declaration)
            .then(|| store.symbols.get(declaration))
        else {
            return Err(JvmCodegenError::InvalidClassDeclaration {
                class,
                symbol: declaration,
            });
        };
        if symbol.owner == Some(class) && symbol.kind == SymbolKind::TypeParameter {
            return Err(JvmCodegenError::UnsupportedJvmClass {
                class,
                kind: UnsupportedJvmClassKind::Generic,
            });
        }
    }
    Ok(())
}

fn validate_class_flags(
    class: dotty_core::SymbolId,
    flags: SymbolFlags,
) -> Result<(), JvmCodegenError> {
    if flags.contains(SymbolFlags::ABSTRACT) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Abstract,
        });
    }
    if flags.contains(SymbolFlags::ENUM) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Enum,
        });
    }
    if flags.contains(SymbolFlags::SYNTHETIC) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Synthetic,
        });
    }
    if flags.contains(SymbolFlags::CASE) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Case,
        });
    }
    if flags.contains(SymbolFlags::SEALED) {
        return Err(JvmCodegenError::UnsupportedJvmClass {
            class,
            kind: UnsupportedJvmClassKind::Sealed,
        });
    }
    for (flag, unsupported) in [
        (SymbolFlags::IMPLICIT, UnsupportedJvmSymbolFlag::Implicit),
        (SymbolFlags::GIVEN, UnsupportedJvmSymbolFlag::Given),
        (SymbolFlags::LAZY, UnsupportedJvmSymbolFlag::Lazy),
        (SymbolFlags::MUTABLE, UnsupportedJvmSymbolFlag::Mutable),
        (SymbolFlags::INLINE, UnsupportedJvmSymbolFlag::Inline),
        (
            SymbolFlags::TRANSPARENT,
            UnsupportedJvmSymbolFlag::Transparent,
        ),
        (SymbolFlags::OPAQUE, UnsupportedJvmSymbolFlag::Opaque),
        (SymbolFlags::EXTENSION, UnsupportedJvmSymbolFlag::Extension),
        (SymbolFlags::STATIC, UnsupportedJvmSymbolFlag::Static),
        (
            SymbolFlags::JAVA_DEFINED,
            UnsupportedJvmSymbolFlag::JavaDefined,
        ),
        (SymbolFlags::ERASED, UnsupportedJvmSymbolFlag::Erased),
        (SymbolFlags::OVERRIDE, UnsupportedJvmSymbolFlag::Override),
        (SymbolFlags::EXPORTED, UnsupportedJvmSymbolFlag::Exported),
    ] {
        if flags.contains(flag) {
            return Err(JvmCodegenError::UnsupportedJvmSymbolFlag {
                symbol: class,
                flag: unsupported,
            });
        }
    }
    Ok(())
}

fn is_simple_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || matches!(first, '_' | '$'))
        && chars.all(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
}

fn invalid_name(
    symbol: dotty_core::SymbolId,
    kind: JvmNameKind,
    text: impl Into<String>,
) -> JvmCodegenError {
    JvmCodegenError::InvalidJvmName {
        symbol,
        kind,
        text: text.into(),
    }
}

fn invalid_class_info(
    class: dotty_core::SymbolId,
    ty: Option<dotty_core::TypeId>,
    kind: InvalidJvmClassInfoKind,
) -> JvmCodegenError {
    JvmCodegenError::InvalidJvmClassInfo { class, ty, kind }
}
