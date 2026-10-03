object CaseArrowNewline:
  def specialize(tree: Tree): Tree = tree match
    case Apply(TypeApply(fun, targs), args)
        if fun.name == "apply"
        && fun.exists
        && isElideableExpr(tree)
    =>
      rebuild(tree)
    case _ => tree
