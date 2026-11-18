//! Scala's BLOCK(LAMBDA, DEFDEF) and the erasure annotation on the enclosing method.

use super::*;
use dotty_core::ast::TypedAst;

impl EncodingContext<'_> {
    pub(crate) fn context_function_int_syntax(&mut self) -> Result<AstLabel, PickleError> {
        let constructor = self.scala_named_type("ContextFunction1")?;
        let input = self.int_type_syntax()?;
        let output = self.int_type_syntax()?;
        self.wire_node(AstNode::Length {
            tag: APPLIEDTPT_TAG,
            fields: vec![
                AstField::Tree(constructor),
                AstField::Tree(input),
                AstField::Tree(output),
            ],
        })
    }

    fn closure_declaration(&mut self, symbol: SymbolId) -> Result<AstLabel, PickleError> {
        self.charge(1)?;
        self.namespace(symbol, false)?;
        if self.symbol(symbol)?.origin != SymbolOrigin::Synthetic {
            return Err(reference(symbol, ReferenceError::NotLocal));
        }
        if self.declarations.contains_key(&symbol) {
            return Err(PickleError::DuplicateDeclaration { symbol });
        }
        let label = self.wire.reserve().map_err(construction)?;
        self.declarations.insert(symbol, label);
        Ok(label)
    }

    pub(crate) fn context_function_body(
        &mut self,
        ast: &TypedAst,
        closure: CheckedUnaryContextClosure,
        parameters: &[SymbolId],
        limits: ProjectionLimits,
    ) -> Result<AstLabel, PickleError> {
        let method = self.closure_declaration(closure.method)?;
        let parameter = self.closure_declaration(closure.parameter)?;
        let reference = self.local_reference(closure.method, None)?;
        let lambda = self.wire_node(AstNode::Length {
            tag: LAMBDA_TAG,
            fields: vec![AstField::Tree(reference)],
        })?;
        let name = self.intern_name(self.symbol(closure.parameter)?.name)?;
        let param_type = self.int_type_syntax()?;
        let given = self.leaf(GIVEN_TAG, TermValue::Tag)?;
        self.bind_declaration(
            closure.parameter,
            AstNode::Length {
                tag: PARAM_TAG,
                fields: vec![
                    AstField::Nat(name),
                    AstField::Tree(param_type),
                    AstField::Tree(given),
                ],
            },
        )?;
        let result = self.lower_type(closure.result)?;
        let body = crate::expression::lower_expression(
            self.store,
            ast,
            closure.body,
            closure.method,
            parameters,
            limits,
            self,
        )?;
        let anon = self.utf8("$anonfun")?;
        let synthetic = self.leaf(SYNTHETIC_TAG, TermValue::Tag)?;
        let artifact = self.leaf(ARTIFACT_TAG, TermValue::Tag)?;
        self.bind_declaration(
            closure.method,
            AstNode::Length {
                tag: DEFDEF_TAG,
                fields: vec![
                    AstField::Nat(anon),
                    AstField::Tree(parameter),
                    AstField::Tree(result),
                    AstField::Tree(body),
                    AstField::Tree(synthetic),
                    AstField::Tree(artifact),
                ],
            },
        )?;
        self.wire_node(AstNode::Length {
            tag: BLOCK_TAG,
            fields: vec![AstField::Tree(lambda), AstField::Tree(method)],
        })
    }

    pub(crate) fn context_result_annotation(&mut self) -> Result<AstLabel, PickleError> {
        let scala = self.utf8("scala")?;
        let ann = self.utf8("annotation")?;
        let internal = self.utf8("internal")?;
        let package = self.intern(NameKey::Qualified(scala, ann))?;
        let package = self.intern(NameKey::Qualified(package, internal))?;
        let name = self.utf8("ContextResultCount")?;
        let qualified = self.intern(NameKey::Qualified(package, name))?;
        let int = self.utf8("Int")?;
        let int = self.intern(NameKey::Qualified(scala, int))?;
        let init = self.utf8("<init>")?;
        let signed = self.intern(NameKey::SignedMany(
            init,
            qualified,
            vec![i32::try_from(int).map_err(|_| PickleError::LimitExceeded {
                kind: LimitKind::Names,
                limit: i32::MAX as usize,
            })?],
        ))?;
        let prefix = self.leaf(TERMREFPKG_TAG, TermValue::NameRef(package))?;
        let tpe = self.wire_node(AstNode::NatAst {
            tag: TYPEREF_TAG,
            value: AstOperand::Nat(name),
            child: prefix,
        })?;
        let fresh = self.wire_node(AstNode::Reference {
            tag: SHAREDTYPE_TAG,
            target: tpe,
        })?;
        let owner = self.wire_node(AstNode::Reference {
            tag: SHAREDTYPE_TAG,
            target: tpe,
        })?;
        let new = self.wire_node(AstNode::Ast {
            tag: NEW_TAG,
            child: fresh,
        })?;
        let select = self.wire_node(AstNode::Length {
            tag: SELECTIN_TAG,
            fields: vec![
                AstField::Nat(signed),
                AstField::Tree(new),
                AstField::Tree(owner),
            ],
        })?;
        let one = self.leaf(INTCONST_TAG, TermValue::Int(1))?;
        let value = self.wire_node(AstNode::Length {
            tag: APPLY_TAG,
            fields: vec![AstField::Tree(select), AstField::Tree(one)],
        })?;
        self.wire_node(AstNode::Length {
            tag: ANNOTATION_TAG,
            fields: vec![AstField::Tree(tpe), AstField::Tree(value)],
        })
    }
}
