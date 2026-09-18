//! Pass 1 for definitions: entering symbols for packages' members, classes,
//! their members and parameters.
//!
//! The traversal follows the AST address index rather than nested payloads:
//! only the index reports *absolute* addresses (a node decoded out of a
//! parent's payload knows only its offset inside that payload), and address
//! is the identity of a definition. Owners are known top-down, so a symbol is
//! always allocated after its owner and no forward reference is involved.
//!
//! Every definition gets a symbol. Only packages and classes get a
//! declaration scope; a definition is entered into its owner's scope when
//! the owner has one and the definition is a member of it. Parameters of a
//! method, and the parameters of a class's constructor node, are owned by
//! that method but are not members of any scope. Definitions inside method
//! bodies (locals) are not entered.

use std::collections::HashMap;

use dotty_core::ids::SymbolId;
use dotty_core::names::Name;
use dotty_core::symbols::{Scope, Symbol, SymbolInfo, SymbolKind, SymbolLinks};
use dotty_tasty::tasty::{
    AstAddressIndex, AstTreeNode, DEFDEF_TAG, DefDefHeaderItem, DefinitionBody, PACKAGE_TAG,
    PARAM_TAG, ParameterNode, RawNode, RawTree, StructuredNode, TEMPLATE_TAG, TYPEDEF_TAG,
    TYPEPARAM_TAG, TastyFile, VALDEF_TAG,
};

use crate::error::UnpickleError;
use crate::mapping::{
    DeclaredModifiers, def_def_kind, namespace_of, term_param_kind, type_def_kind, type_param_kind,
    val_def_kind,
};
use crate::names::{qualified_segments, wire_name};
use crate::unpickler::TastyUnpickler;

/// The file's AST as the enter pass walks it: nodes by absolute address, and
/// each node's direct children in wire order.
pub(crate) struct AstView<'bytes> {
    index: AstAddressIndex<'bytes>,
    children: HashMap<u32, Vec<AstTreeNode>>,
}

impl<'bytes> AstView<'bytes> {
    pub fn new(file: &TastyFile<'bytes>) -> Result<Self, UnpickleError> {
        let index = file.ast_address_index()?;
        let mut children: HashMap<u32, Vec<AstTreeNode>> = HashMap::new();
        for edge in index.iter_tree_edges() {
            children
                .entry(address(edge.parent.offset))
                .or_default()
                .push(edge.child);
        }
        Ok(Self { index, children })
    }

    fn node(&self, at: u32) -> Result<&RawNode<'bytes>, UnpickleError> {
        self.index
            .get(at)
            .ok_or(UnpickleError::MissingDefinition { address: at })
    }

    fn children(&self, at: u32) -> &[AstTreeNode] {
        self.children.get(&at).map_or(&[], Vec::as_slice)
    }
}

/// What is known about a definition when its symbol is allocated.
struct Declaration<'a> {
    at: u32,
    tag: u8,
    name_ref: u32,
    kind: SymbolKind,
    modifiers: &'a DeclaredModifiers,
    owner: SymbolId,
    /// Whether the symbol is a member of its owner's declaration scope.
    member: bool,
}

/// The AST address for an absolute offset. TASTy addresses fit `u32`; a
/// larger offset cannot name a node.
fn address(offset: usize) -> u32 {
    u32::try_from(offset).unwrap_or(u32::MAX)
}

impl TastyUnpickler<'_, '_, '_> {
    /// Enters a `PACKAGE` node: its package symbol, then its members.
    pub(crate) fn enter_package(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
    ) -> Result<(), UnpickleError> {
        let package = ast.node(at)?.decode_package()?;
        let path_name = package
            .path_name()
            .ok_or(UnpickleError::UnsupportedPackagePath { address: at })?;
        let path = qualified_segments(self.file.names(), path_name)?;

        let symbol = self
            .packages
            .enter(self.store, &mut self.index, self.origin, &path)?;
        self.index.insert_symbol(at, symbol)?;

        for child in ast.children(at) {
            let child_at = address(child.offset);
            match child.tag {
                PACKAGE_TAG => self.enter_package(ast, child_at)?,
                TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG => {
                    self.enter_definition(ast, child_at, child.tag, symbol)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Enters a `TYPEDEF`, `VALDEF` or `DEFDEF` owned by `owner`, and the
    /// definitions nested in it.
    fn enter_definition(
        &mut self,
        ast: &AstView<'_>,
        at: u32,
        tag: u8,
        owner: SymbolId,
    ) -> Result<(), UnpickleError> {
        let node = ast.node(at)?;
        let owner_kind = self.store.symbols.get(owner).kind;

        match node.decode_structured()? {
            StructuredNode::TypeDef(DefinitionBody::TypeDef {
                name,
                type_or_template,
                tail,
            }) => {
                let modifiers = DeclaredModifiers::from_tail(&tail)?;
                let has_template = matches!(
                    &type_or_template,
                    RawTree::LengthNode(template) if template.tag == TEMPLATE_TAG
                );
                let kind = type_def_kind(&modifiers, has_template);
                let symbol = self.enter_symbol(Declaration {
                    at,
                    tag,
                    name_ref: name,
                    kind,
                    modifiers: &modifiers,
                    owner,
                    member: true,
                })?;
                if let RawTree::LengthNode(template) = &type_or_template
                    && template.tag == TEMPLATE_TAG
                {
                    self.enter_class_scope(symbol)?;
                    self.enter_template(ast, at, symbol, template)?;
                }
            }
            StructuredNode::ValDef(DefinitionBody::ValDef { name, tail, .. }) => {
                let modifiers = DeclaredModifiers::from_tail(&tail)?;
                let kind = val_def_kind(&modifiers, owner_kind);
                self.enter_symbol(Declaration {
                    at,
                    tag,
                    name_ref: name,
                    kind,
                    modifiers: &modifiers,
                    owner,
                    member: true,
                })?;
            }
            StructuredNode::DefDef(body) => {
                let modifiers = DeclaredModifiers::from_tail(&body.tail)?;
                let kind = def_def_kind(&wire_name(self.file.names(), body.name)?);
                let method = self.enter_symbol(Declaration {
                    at,
                    tag,
                    name_ref: body.name,
                    kind,
                    modifiers: &modifiers,
                    owner,
                    member: true,
                })?;
                let parameters: Vec<&ParameterNode<'_>> = body
                    .header_items
                    .iter()
                    .filter_map(|item| match item {
                        DefDefHeaderItem::Parameter(parameter) => Some(parameter),
                        DefDefHeaderItem::Clause(_) => None,
                    })
                    .collect();
                self.enter_parameters(ast, at, method, &parameters)?;
            }
            _ => return Err(UnpickleError::MissingDefinition { address: at }),
        }
        Ok(())
    }

    /// Enters the members of the class whose node is at `class_at`: the
    /// class's own type parameters and constructor parameters (its template
    /// header), then its statements.
    fn enter_template(
        &mut self,
        ast: &AstView<'_>,
        class_at: u32,
        class: SymbolId,
        template: &RawNode<'_>,
    ) -> Result<(), UnpickleError> {
        let Some(template_node) = ast
            .children(class_at)
            .iter()
            .find(|child| child.tag == TEMPLATE_TAG)
        else {
            return Err(UnpickleError::MissingDefinition { address: class_at });
        };
        let template_at = address(template_node.offset);
        let structure = template.decode_template_structure()?;
        let parameters: Vec<&ParameterNode<'_>> = structure
            .type_params
            .iter()
            .chain(structure.term_params.iter())
            .collect();

        self.enter_paired_parameters(ast, template_at, class, &parameters, true)?;
        for child in ast.children(template_at) {
            if matches!(child.tag, TYPEDEF_TAG | VALDEF_TAG | DEFDEF_TAG) {
                self.enter_definition(ast, address(child.offset), child.tag, class)?;
            }
            // Parents, the self definition and non-definition statements
            // declare no symbol.
        }
        Ok(())
    }

    /// Enters the parameters of the method at `method_at`.
    fn enter_parameters(
        &mut self,
        ast: &AstView<'_>,
        method_at: u32,
        method: SymbolId,
        parameters: &[&ParameterNode<'_>],
    ) -> Result<(), UnpickleError> {
        self.enter_paired_parameters(ast, method_at, method, parameters, false)
    }

    /// Enters the `TYPEPARAM`/`PARAM` nodes directly under `parent_at`.
    ///
    /// The AST index knows each parameter's absolute address but its payload
    /// for a parameter node omits the name, so the name and modifiers come
    /// from the parent's structural decoding (`parameters`). The two are
    /// paired in wire order, and a disagreement is an error rather than a
    /// guess.
    fn enter_paired_parameters(
        &mut self,
        ast: &AstView<'_>,
        parent_at: u32,
        owner: SymbolId,
        parameters: &[&ParameterNode<'_>],
        in_class: bool,
    ) -> Result<(), UnpickleError> {
        let nodes: Vec<&AstTreeNode> = ast
            .children(parent_at)
            .iter()
            .filter(|child| matches!(child.tag, TYPEPARAM_TAG | PARAM_TAG))
            .collect();
        let mismatch = UnpickleError::ParameterMismatch { address: parent_at };
        if nodes.len() != parameters.len() {
            return Err(mismatch);
        }

        for (node, parameter) in nodes.into_iter().zip(parameters) {
            if node.tag != parameter.tag() {
                return Err(mismatch);
            }
            self.enter_parameter(address(node.offset), parameter, owner, in_class)?;
        }
        Ok(())
    }

    /// Enters one parameter. In a class header (`in_class`) a parameter that
    /// is a member is also declared in the class scope.
    fn enter_parameter(
        &mut self,
        at: u32,
        parameter: &ParameterNode<'_>,
        owner: SymbolId,
        in_class: bool,
    ) -> Result<(), UnpickleError> {
        let tag = parameter.tag();
        let modifiers = DeclaredModifiers::from_tail(&parameter.decode_body()?.tail)?;
        let kind = if tag == TYPEPARAM_TAG {
            type_param_kind()
        } else {
            term_param_kind(&modifiers, in_class)
        };
        let is_member = in_class && matches!(kind, SymbolKind::TypeParameter | SymbolKind::Field);
        self.enter_symbol(Declaration {
            at,
            tag,
            name_ref: parameter.name(),
            kind,
            modifiers: &modifiers,
            owner,
            member: is_member,
        })?;
        Ok(())
    }

    /// Allocates the symbol for a definition, records its address, and
    /// declares it in its owner's scope when it is a member and the owner has
    /// one.
    fn enter_symbol(&mut self, declaration: Declaration<'_>) -> Result<SymbolId, UnpickleError> {
        let Declaration {
            at,
            tag,
            name_ref,
            kind,
            modifiers,
            owner,
            member,
        } = declaration;
        let namespace =
            namespace_of(tag).ok_or(UnpickleError::MissingDefinition { address: at })?;
        let text = wire_name(self.file.names(), name_ref)?;
        let name = Name::new(self.store.names.intern(&text), namespace);

        let symbol = self.store.symbols.alloc(Symbol {
            name,
            owner: Some(owner),
            kind,
            flags: modifiers.flags,
            visibility: modifiers.visibility,
            info: SymbolInfo::Missing,
            origin: self.origin,
            annotations: Vec::new(),
            position: None,
            links: SymbolLinks::default(),
        });
        self.index.insert_symbol(at, symbol)?;

        if member && let Some(scope) = self.index.scope_of(owner) {
            self.store.scopes.get_mut(scope).enter(name, symbol);
        }
        Ok(symbol)
    }

    fn enter_class_scope(&mut self, class: SymbolId) -> Result<(), UnpickleError> {
        let scope = self.store.scopes.alloc(Scope::new(Some(class)));
        self.index.insert_scope(class, scope)
    }
}
