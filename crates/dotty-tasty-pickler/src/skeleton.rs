//! Shared declaration assembly and explicitly incomplete skeleton output.

use dotty_core::{Name, Symbol, SymbolId, SymbolInfo, TreeId, Type, Typed};
use dotty_tasty::tasty::*;

use crate::{
    EncodingContext, PickleError, ProjectionLimits, ReferenceError, SourceMetadata, TypeRole,
    ValidatedPicklePlan,
};

/// Canonical declaration identity bound to a node start in the ASTs section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkeletonDeclaration {
    pub symbol: SymbolId,
    pub owner: SymbolId,
    pub address: u32,
}

/// A real typed RHS deliberately not serialized by this declaration increment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OmittedMethodBody {
    pub method: SymbolId,
    pub typed_root: TreeId<Typed>,
}

/// Structurally validated, deliberately incomplete TASTy declaration document.
///
/// The absent concrete RHS is not an abstract source declaration or a fake body.
/// Do not publish these bytes as a completed compiler artifact or pass them to
/// Scala expecting interoperability. Use `ValidatedPicklePlan::pickle` for complete literal-body emission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclarationSkeleton {
    bytes: Vec<u8>,
    source: SourceMetadata,
    intended_tasty_path: String,
    declarations: Vec<SkeletonDeclaration>,
    omitted_bodies: Vec<OmittedMethodBody>,
}

/// Private common assembly result; completeness is selected before emission.
pub(crate) struct Assembly {
    pub bytes: Vec<u8>,
    pub source: SourceMetadata,
    pub intended_tasty_path: String,
    pub declarations: Vec<SkeletonDeclaration>,
    pub bodies: Vec<(SymbolId, TreeId<Typed>)>,
}

impl DeclarationSkeleton {
    /// Provisional document bytes, suitable for structural inspection only.
    pub fn provisional_bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Source provenance retained separately from the package-derived output path.
    pub fn source(&self) -> &SourceMetadata {
        &self.source
    }
    /// Intended path with Scala-encoded package/class components; writes no files.
    pub fn intended_tasty_path(&self) -> &str {
        &self.intended_tasty_path
    }
    /// Class, constructor, then ordinary methods in canonical source order.
    pub fn declarations(&self) -> &[SkeletonDeclaration] {
        &self.declarations
    }
    /// Nonempty in this slice: every concrete source method RHS remains pending.
    pub fn omitted_bodies(&self) -> &[OmittedMethodBody] {
        &self.omitted_bodies
    }
}

fn encoding(error: impl Into<TastyFileError>) -> PickleError {
    PickleError::Encoding {
        cause: Box::new(error.into()),
    }
}

fn marker(context: &mut EncodingContext<'_>, tag: u8) -> Result<AstLabel, PickleError> {
    let term = SimpleTerm::new(tag, TermValue::Tag).map_err(encoding)?;
    context.wire_node(AstNode::Leaf(term))
}

fn symbol<'a>(plan: &ValidatedPicklePlan<'_, 'a>, id: SymbolId) -> Result<&'a Symbol, PickleError> {
    let store = plan.plan().store;
    if !store.symbols.contains(id) {
        return Err(PickleError::InvalidSymbol { symbol: id });
    }
    Ok(store.symbols.get(id))
}

fn text<'a>(plan: &ValidatedPicklePlan<'_, 'a>, name: Name) -> Result<&'a str, PickleError> {
    plan.plan()
        .store
        .names
        .try_resolve(name.text())
        .ok_or(PickleError::InvalidName { name: name.text() })
}

impl ValidatedPicklePlan<'_, '_> {
    /// Emits only the frozen one-class declaration skeleton, with real method
    /// bodies explicitly recorded as omitted. It never publishes a file.
    pub fn declaration_skeleton(&self) -> Result<DeclarationSkeleton, PickleError> {
        self.declaration_skeleton_with_limits(ProjectionLimits::default())
    }

    /// Uses the existing projection/name/wire budgets. The bounded v0 shape has
    /// one package, one public class, a default constructor and one or more methods.
    pub fn declaration_skeleton_with_limits(
        &self,
        limits: ProjectionLimits,
    ) -> Result<DeclarationSkeleton, PickleError> {
        let output = self.assemble(limits, None)?;
        Ok(DeclarationSkeleton {
            bytes: output.bytes,
            source: output.source,
            intended_tasty_path: output.intended_tasty_path,
            declarations: output.declarations,
            omitted_bodies: output
                .bodies
                .into_iter()
                .map(|(method, typed_root)| OmittedMethodBody { method, typed_root })
                .collect(),
        })
    }

    pub(crate) fn assemble(
        &self,
        limits: ProjectionLimits,
        complete: Option<usize>,
    ) -> Result<Assembly, PickleError> {
        let p = self.plan();
        let mut context = EncodingContext::for_plan(self, limits)?;
        let package_path = context.package_reference(self.package())?;
        let class_record = symbol(self, p.class)?;
        let class_info = match class_record.info {
            SymbolInfo::Complete(ty) => ty,
            info => {
                return Err(PickleError::IncompleteSymbol {
                    symbol: p.class,
                    info,
                });
            }
        };
        let Some(Type::ClassInfo(info)) = p.store.types.try_get(class_info) else {
            return Err(PickleError::MalformedType {
                ty: class_info,
                role: TypeRole::ClassInfo,
            });
        };
        let [parent] = info.parents.as_slice() else {
            return Err(PickleError::MalformedType {
                ty: class_info,
                role: TypeRole::ClassInfo,
            });
        };
        // The validated ClassInfo supplies the canonical implicit Object type.
        // Its constructor application is the frozen Scala oracle's wire form.
        let parent_type = context.lower_type(*parent)?;
        let new_parent = context.wire_node(AstNode::Ast {
            tag: NEW_TAG,
            child: parent_type,
        })?;
        let constructor_name = context.object_constructor_name()?;
        let owner_type = context.lower_type(*parent)?;
        let select = context.wire_node(AstNode::Length {
            tag: SELECTIN_TAG,
            fields: vec![
                AstField::Nat(constructor_name),
                AstField::Tree(new_parent),
                AstField::Tree(owner_type),
            ],
        })?;
        let parent_application = context.wire_node(AstNode::Length {
            tag: APPLY_TAG,
            fields: vec![AstField::Tree(select)],
        })?;

        let name = context.intern_name(symbol(self, p.constructor.symbol)?.name)?;
        let empty_clause = marker(&mut context, EMPTYCLAUSE_TAG)?;
        // Unit is wire return syntax only: the semantic constructor signature
        // still returns its owning class. Default-constructor provenance was
        // checked in the source plan; its required STABLE is not a core flag.
        let unit = context.constructor_return_type()?;
        let stable = marker(&mut context, STABLE_TAG)?;
        context.bind_declaration(
            p.constructor.symbol,
            AstNode::Length {
                tag: DEFDEF_TAG,
                fields: vec![
                    AstField::Nat(name),
                    AstField::Tree(empty_clause),
                    AstField::Tree(unit),
                    AstField::Tree(stable),
                ],
            },
        )?;
        let constructor_label =
            context
                .declaration_label(p.constructor.symbol)
                .ok_or(PickleError::Reference {
                    symbol: p.constructor.symbol,
                    reason: ReferenceError::NotRegistered,
                })?;
        let mut stats = vec![
            AstField::Tree(parent_application),
            AstField::Tree(constructor_label),
        ];
        let mut bodies = Vec::new();
        for method in p.methods {
            let name = context.intern_name(symbol(self, method.symbol)?.name)?;
            // Preserve the source distinction: no clause has no marker; one
            // explicit empty clause has EMPTYCLAUSE before the result type.
            let mut fields = vec![AstField::Nat(name)];
            if method.parameter_clauses.len() == 1 {
                let parameters = &method.parameter_clauses[0];
                if parameters.is_empty() {
                    fields.push(AstField::Tree(marker(&mut context, EMPTYCLAUSE_TAG)?));
                } else {
                    for &parameter in parameters {
                        let parameter_name = context.intern_name(symbol(self, parameter)?.name)?;
                        let parameter_type = context.int_type_syntax()?;
                        context.bind_declaration(
                            parameter,
                            AstNode::Length {
                                tag: PARAM_TAG,
                                fields: vec![
                                    AstField::Nat(parameter_name),
                                    AstField::Tree(parameter_type),
                                ],
                            },
                        )?;
                        fields.push(AstField::Tree(context.declaration_label(parameter).ok_or(
                            PickleError::Reference {
                                symbol: parameter,
                                reason: ReferenceError::NotRegistered,
                            },
                        )?));
                    }
                }
            }
            let result = context.int_type_syntax()?;
            // No invented METHODtype payload, abstract modifier or substitute
            // for the method's real RHS.
            fields.push(AstField::Tree(result));
            if complete.is_some() {
                let body = method.body.ok_or(PickleError::MissingBody {
                    method: method.symbol,
                })?;
                fields.push(AstField::Tree(crate::expression::lower_expression(
                    p.store,
                    p.typed_ast,
                    body,
                    method.symbol,
                    &mut context,
                )?));
            }
            context.bind_declaration(
                method.symbol,
                AstNode::Length {
                    tag: DEFDEF_TAG,
                    fields,
                },
            )?;
            let label = context
                .declaration_label(method.symbol)
                .ok_or(PickleError::Reference {
                    symbol: method.symbol,
                    reason: ReferenceError::NotRegistered,
                })?;
            stats.push(AstField::Tree(label));
            bodies.push((
                method.symbol,
                method.body.ok_or(PickleError::MissingBody {
                    method: method.symbol,
                })?,
            ));
        }
        // All supported semantic visibility/flags/annotations are public/empty,
        // as validated from symbol records, not Typed's unit metadata. Unsupported
        // flags cannot reach here. The compiler SourceFile annotation is omitted
        // under the independently tested reduced-oracle contract.
        let template = context.wire_node(AstNode::Length {
            tag: TEMPLATE_TAG,
            fields: stats,
        })?;
        let name = context.intern_name(class_record.name)?;
        context.bind_declaration(
            p.class,
            AstNode::Length {
                tag: TYPEDEF_TAG,
                fields: vec![AstField::Nat(name), AstField::Tree(template)],
            },
        )?;
        let class_label = context
            .declaration_label(p.class)
            .ok_or(PickleError::Reference {
                symbol: p.class,
                reason: ReferenceError::NotRegistered,
            })?;
        let package = context.wire_node(AstNode::Length {
            tag: PACKAGE_TAG,
            fields: vec![AstField::Tree(package_path), AstField::Tree(class_label)],
        })?;
        let asts_name = context.intern_wire_text("ASTs")?;
        let attributes_name = context.intern_wire_text("Attributes")?;
        let source_name = context.intern_wire_text(&p.source.name)?;
        let fragment = context.finish(&[package])?;
        let mut declarations = Vec::new();
        for id in std::iter::once(p.class)
            .chain(std::iter::once(p.constructor.symbol))
            .chain(p.methods.iter().map(|m| m.symbol))
            .chain(
                p.methods
                    .iter()
                    .flat_map(|method| method.parameter_clauses.iter().flatten().copied()),
            )
        {
            let label = context
                .declaration_label(id)
                .ok_or(PickleError::Reference {
                    symbol: id,
                    reason: ReferenceError::NotRegistered,
                })?;
            let address = fragment.ast.address(label).ok_or(PickleError::Reference {
                symbol: id,
                reason: ReferenceError::NotRegistered,
            })?;
            let owner = symbol(self, id)?.owner.ok_or(PickleError::Reference {
                symbol: id,
                reason: ReferenceError::Owner,
            })?;
            declarations.push(SkeletonDeclaration {
                symbol: id,
                owner,
                address,
            });
        }
        let mut header = Header {
            major_version: SCALA_3_9_MAJOR_VERSION,
            minor_version: SCALA_3_9_MINOR_VERSION,
            experimental_version: SCALA_3_9_EXPERIMENTAL_VERSION,
            tooling_version: "dotty-rs incomplete declaration skeleton".into(),
            // A fixed diagnostic UUID, not a completed artifact content identity.
            uuid: [0; 16],
        };
        let asts = EncodedSection::raw(asts_name, fragment.ast.bytes());
        let attributes =
            EncodedSection::attributes(attributes_name, &[Attribute::SourceFile(source_name)])
                .map_err(encoding)?;
        if let Some(max_file_bytes) = complete {
            header.tooling_version = "dotty-rs minimal semantic pickler".into();
            crate::output::prepare_header(
                &mut header,
                &fragment.names,
                &asts,
                &attributes,
                max_file_bytes,
            )?;
        }
        let builder = TastyFileBuilder::new(header, fragment.names)
            .with_section(asts)
            .with_section(attributes);
        builder.validate_scala_3_9().map_err(encoding)?;
        let bytes = builder.encode_validated().map_err(encoding)?;
        let intended_tasty_path = format!(
            "{}/{}.tasty",
            crate::artifact_name::encode_component(text(self, symbol(self, self.package())?.name)?),
            crate::artifact_name::encode_component(text(self, class_record.name)?)
        );
        Ok(Assembly {
            bytes,
            source: p.source.clone(),
            intended_tasty_path,
            declarations,
            bodies,
        })
    }
}
