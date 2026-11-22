//! Source-type components of Scala's invented names for anonymous givens.
use super::{AstArena, TreeKind, Untyped, UntypedNode};
use crate::TreeId;
use std::collections::HashSet;

/// The first source tree which cannot contribute an invented name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsupportedGivenNameTree {
    /// Source tree identity in its original session arena.
    pub tree: TreeId<Untyped>,
    /// Stable diagnostic label for the unsupported syntax or type shape.
    pub kind: &'static str,
}

/// Diagnostic label for a source type-name component.
pub fn given_type_tree_kind(tree: &TreeKind<Untyped>) -> &'static str {
    match tree {
        TreeKind::Ident(_) => "Ident",
        TreeKind::Select(_) => "Select",
        TreeKind::TypeTree(_) => "TypeTree",
        TreeKind::SingletonTypeTree(_) => "SingletonTypeTree",
        TreeKind::RefinedTypeTree(_) => "RefinedTypeTree",
        TreeKind::MatchTypeTree(_) => "MatchTypeTree",
        TreeKind::ByNameTypeTree(_) => "ByNameTypeTree",
        TreeKind::TypeBoundsTree(_) => "TypeBoundsTree",
        TreeKind::This(_) => "This",
        TreeKind::Super(_) => "Super",
        TreeKind::Literal(_) => "Literal",
        TreeKind::Apply(_) => "Apply",
        TreeKind::TypeApply(_) => "TypeApply",
        TreeKind::New(_) => "New",
        TreeKind::Typed(_) => "Typed",
        TreeKind::AppliedTypeTree(_) => "AppliedTypeTree",
        TreeKind::LambdaTypeTree(_) => "LambdaTypeTree",
        TreeKind::NamedArg(_) => "NamedArg",
        TreeKind::Assign(_) => "Assign",
        TreeKind::Block(_) => "Block",
        TreeKind::If(_) => "If",
        TreeKind::Match(_) => "Match",
        TreeKind::CaseDef(_) => "CaseDef",
        TreeKind::Return(_) => "Return",
        TreeKind::While(_) => "While",
        TreeKind::Try(_) => "Try",
        TreeKind::Closure(_) => "Closure",
        TreeKind::ValDef(_) => "ValDef",
        TreeKind::DefDef(_) => "DefDef",
        TreeKind::TypeDef(_) => "TypeDef",
        TreeKind::Template(_) => "Template",
        TreeKind::PackageDef(_) => "PackageDef",
        TreeKind::Import(_) => "Import",
        TreeKind::Export(_) => "Export",
        TreeKind::PhaseSpecific(UntypedNode::Error(_)) => "Error",
        TreeKind::PhaseSpecific(UntypedNode::ModuleDef(_)) => "ModuleDef",
        TreeKind::PhaseSpecific(UntypedNode::PolyFunction(_)) => "PolyFunction",
        TreeKind::PhaseSpecific(UntypedNode::PrefixOp(_)) => "PrefixOp",
        TreeKind::PhaseSpecific(UntypedNode::PostfixOp(_)) => "PostfixOp",
        TreeKind::PhaseSpecific(UntypedNode::ForYield(_)) => "ForYield",
        TreeKind::PhaseSpecific(UntypedNode::ForDo(_)) => "ForDo",
        TreeKind::PhaseSpecific(UntypedNode::GenFrom(_)) => "GenFrom",
        TreeKind::PhaseSpecific(UntypedNode::GenAlias(_)) => "GenAlias",
        TreeKind::PhaseSpecific(UntypedNode::PatDef(_)) => "PatDef",
        TreeKind::PhaseSpecific(UntypedNode::ExtensionMethods(_)) => "ExtensionMethods",
        TreeKind::PhaseSpecific(UntypedNode::InterpolatedString(_)) => "InterpolatedString",
        TreeKind::PhaseSpecific(UntypedNode::ContextBounds(_)) => "ContextBounds",
        TreeKind::PhaseSpecific(UntypedNode::Number(_)) => "Number",
        TreeKind::PhaseSpecific(UntypedNode::Throw(_)) => "Throw",
        TreeKind::PhaseSpecific(UntypedNode::InlineIf(_)) => "InlineIf",
        TreeKind::PhaseSpecific(UntypedNode::InlineMatch(_)) => "InlineMatch",
        TreeKind::PhaseSpecific(UntypedNode::MacroTree(_)) => "MacroTree",
        TreeKind::PhaseSpecific(UntypedNode::ParsedTry(_)) => "ParsedTry",
        TreeKind::PhaseSpecific(UntypedNode::Function(_)) => "Function",
        TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(_)) => "FunctionWithMods",
        TreeKind::PhaseSpecific(UntypedNode::CapturesAndResult(_)) => "CapturesAndResult",
        TreeKind::PhaseSpecific(UntypedNode::InfixOp(_)) => "InfixOp",
        TreeKind::PhaseSpecific(UntypedNode::Parens(_)) => "Parens",
        TreeKind::PhaseSpecific(UntypedNode::Tuple(_)) => "Tuple",
        TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(_)) => "ContextBoundTypeTree",
        TreeKind::Bind(_) => "Bind",
        TreeKind::Alternative(_) => "Alternative",
        TreeKind::UnApply(_) => "UnApply",
        TreeKind::Annotated(_) => "Annotated",
        TreeKind::Quote(_) => "Quote",
        TreeKind::Splice(_) => "Splice",
        TreeKind::QuotePattern(_) => "QuotePattern",
        TreeKind::SplicePattern(_) => "SplicePattern",
        TreeKind::Inlined(_) => "Inlined",
    }
}

/// Computes the source-derived suffix for an anonymous given. At the root,
/// applied type arguments contribute their leading names.
pub fn extract_given_type_name(
    arena: &AstArena<Untyped>,
    names: &crate::NameInterner,
    tree: TreeId<Untyped>,
    follow_args: bool,
) -> Result<String, UnsupportedGivenNameTree> {
    enum Task<'a> {
        Visit(TreeId<Untyped>, bool),
        Arguments(&'a [TreeId<Untyped>]),
        Text(&'a str),
        Finish(TreeId<Untyped>, usize),
    }

    fn append_reference(output: &mut String, name: crate::Name, names: &crate::NameInterner) {
        output.push_str(names.resolve(name.text()));
        if name.is_term() {
            output.push_str("_type");
        }
    }

    let mut output = String::new();
    let mut active = HashSet::new();
    let mut pending = vec![Task::Visit(tree, follow_args)];
    while let Some(task) = pending.pop() {
        let (tree, follow_args) = match task {
            Task::Text(text) => {
                output.push_str(text);
                continue;
            }
            Task::Arguments(args) => {
                for (ordinal, arg) in args.iter().enumerate().rev() {
                    pending.push(Task::Visit(*arg, false));
                    if ordinal > 0 {
                        pending.push(Task::Text("_"));
                    }
                }
                continue;
            }
            Task::Finish(tree, start) => {
                active.remove(&tree);
                if output.len() == start {
                    return Err(UnsupportedGivenNameTree {
                        tree,
                        kind: given_type_tree_kind(&arena.get(tree).kind),
                    });
                }
                continue;
            }
            Task::Visit(tree, follow_args) => (tree, follow_args),
        };
        if !active.insert(tree) {
            return Err(UnsupportedGivenNameTree {
                tree,
                kind: "cyclic type tree",
            });
        }
        // Append each component once. Recursive string concatenation copies
        // the accumulated name at every level of an applied/function type.
        pending.push(Task::Finish(tree, output.len()));
        let node = arena.try_get(tree).ok_or(UnsupportedGivenNameTree {
            tree,
            kind: "invalid type tree",
        })?;
        match &node.kind {
            TreeKind::Ident(ident) => append_reference(&mut output, ident.name, names),
            TreeKind::Select(select) if names.resolve(select.name.text()) == "<init>" => {
                pending.push(Task::Visit(select.qualifier, follow_args));
            }
            TreeKind::Select(select) => append_reference(&mut output, select.name, names),
            TreeKind::TypeDef(definition) => {
                output.push_str(names.resolve(definition.name.as_name().text()));
            }
            TreeKind::AppliedTypeTree(applied) => {
                if follow_args && !applied.args.is_empty() {
                    pending.push(Task::Arguments(&applied.args));
                    pending.push(Task::Text("_"));
                }
                pending.push(Task::Visit(applied.tpt, follow_args));
            }
            // A constructor application contributes the instantiated parent
            // type. Only AppliedTypeTree arguments are part of its name.
            TreeKind::Apply(applied) => {
                pending.push(Task::Visit(applied.function, follow_args));
            }
            TreeKind::New(new_tree) => {
                pending.push(Task::Visit(new_tree.tpt, follow_args));
            }
            TreeKind::LambdaTypeTree(lambda) => {
                pending.push(Task::Visit(lambda.body, follow_args));
            }
            TreeKind::RefinedTypeTree(refined) if refined.refinements.is_empty() => {
                pending.push(Task::Visit(refined.tpt, follow_args));
            }
            TreeKind::PhaseSpecific(UntypedNode::Parens(parens)) => {
                pending.push(Task::Visit(parens.inner, follow_args));
            }
            TreeKind::PhaseSpecific(UntypedNode::Tuple(tuple)) => {
                pending.push(Task::Arguments(&tuple.elements));
            }
            TreeKind::PhaseSpecific(UntypedNode::Function(function))
                if !function.params.is_empty() =>
            {
                if follow_args {
                    pending.push(Task::Visit(function.body, true));
                    pending.push(Task::Text("_to_"));
                    pending.push(Task::Arguments(&function.params));
                } else {
                    output.push_str("Function");
                }
            }
            TreeKind::PhaseSpecific(UntypedNode::FunctionWithMods(function))
                if !function.params.is_empty() =>
            {
                if follow_args {
                    pending.push(Task::Visit(function.result, true));
                    pending.push(Task::Text("_to_"));
                    pending.push(Task::Arguments(&function.params));
                } else {
                    output.push_str("Function");
                }
            }
            TreeKind::PhaseSpecific(UntypedNode::CapturesAndResult(captures)) => {
                pending.push(Task::Visit(captures.result, follow_args));
            }
            TreeKind::PhaseSpecific(UntypedNode::InfixOp(infix)) => {
                output.push_str(names.resolve(infix.op.text()));
                if follow_args {
                    pending.push(Task::Visit(infix.right, false));
                    pending.push(Task::Text("_"));
                    pending.push(Task::Visit(infix.left, false));
                    pending.push(Task::Text("_"));
                }
            }
            TreeKind::PhaseSpecific(UntypedNode::ContextBoundTypeTree(bound)) => {
                pending.push(Task::Text(names.resolve(bound.parameter.as_name().text())));
                pending.push(Task::Text("_"));
                pending.push(Task::Visit(bound.bound, true));
            }
            node => {
                return Err(UnsupportedGivenNameTree {
                    tree,
                    kind: given_type_tree_kind(node),
                });
            }
        }
    }
    Ok(output)
}
