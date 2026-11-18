//! Scala 3.9 default getter names, copied parameters and unchecked result syntax.
use super::*;
use crate::plan::ContextualDefault;

impl EncodingContext<'_> {
    pub(crate) fn contextual_default_getter(
        &mut self,
        plan: &ValidatedPicklePlan<'_, '_>,
        default: ContextualDefault,
        method_name: NameRef,
        previous_type: Option<AstLabel>,
        complete: bool,
        limits: ProjectionLimits,
    ) -> Result<AstLabel, PickleError> {
        let p = plan.plan();
        let getter = self.reserve_derived(
            default.getter,
            p.class,
            p.source.id,
            default.default_tree,
            SymbolKind::Method,
            SymbolFlags::SYNTHETIC,
        )?;
        let getter_name = self.intern(NameKey::DefaultGetter(method_name, default.ordinal()))?;
        let mut fields = vec![AstField::Nat(getter_name)];
        if let Some(previous) = default.previous {
            let copied = self.reserve_derived(
                previous.copied,
                default.getter,
                p.source.id,
                previous.source_tree,
                SymbolKind::Parameter,
                SymbolFlags::EMPTY,
            )?;
            let name = self.intern_name(self.symbol(previous.copied)?.name)?;
            let tpt = self.wire_node(AstNode::Reference {
                tag: SHAREDTERM_TAG,
                target: previous_type.ok_or(PickleError::SourceDefinition {
                    symbol: previous.copied,
                })?,
            })?;
            self.bind_declaration(
                previous.copied,
                AstNode::Length {
                    tag: PARAM_TAG,
                    fields: vec![AstField::Nat(name), AstField::Tree(tpt)],
                },
            )?;
            fields.push(AstField::Tree(copied));
        }
        fields.push(AstField::Tree(self.unchecked_int_result()?));
        if complete {
            let body = crate::expression::lower_expression(
                p.store,
                p.typed_ast,
                default.body,
                default.getter,
                default.previous.map(|p| p.copied).as_slice(),
                limits,
                self,
            )?;
            fields.push(AstField::Tree(body));
        }
        self.bind_declaration(
            default.getter,
            AstNode::Length {
                tag: DEFDEF_TAG,
                fields,
            },
        )?;
        Ok(getter)
    }

    fn unchecked_int_result(&mut self) -> Result<AstLabel, PickleError> {
        let scala = self.utf8("scala")?;
        let annotation = self.utf8("annotation")?;
        let unchecked = self.utf8("unchecked")?;
        let package = self.intern(NameKey::Qualified(scala, annotation))?;
        let package = self.intern(NameKey::Qualified(package, unchecked))?;
        let name = self.utf8("uncheckedVariance")?;
        let qualified = self.intern(NameKey::Qualified(package, name))?;
        let init = self.utf8("<init>")?;
        let signed = self.intern(NameKey::SignedEmpty(init, qualified))?;
        let prefix = self.leaf(TERMREFPKG_TAG, TermValue::NameRef(package))?;
        let ty = self.wire_node(AstNode::NatAst {
            tag: TYPEREF_TAG,
            value: AstOperand::Nat(name),
            child: prefix,
        })?;
        let new = self.wire_node(AstNode::Ast {
            tag: NEW_TAG,
            child: ty,
        })?;
        let owner = self.wire_node(AstNode::Reference {
            tag: SHAREDTYPE_TAG,
            target: ty,
        })?;
        let select = self.wire_node(AstNode::Length {
            tag: SELECTIN_TAG,
            fields: vec![
                AstField::Nat(signed),
                AstField::Tree(new),
                AstField::Tree(owner),
            ],
        })?;
        let application = self.wire_node(AstNode::Length {
            tag: APPLY_TAG,
            fields: vec![AstField::Tree(select)],
        })?;
        let underlying = self.lower_type(self.definitions.int)?;
        self.wire_node(AstNode::Length {
            tag: ANNOTATEDTYPE_TAG,
            fields: vec![AstField::Tree(underlying), AstField::Tree(application)],
        })
    }
}
