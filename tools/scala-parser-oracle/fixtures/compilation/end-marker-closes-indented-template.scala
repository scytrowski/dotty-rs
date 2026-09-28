trait Outer:
  def updateTracked(tree: Tree): Tree =
    tree match
      case member: MemberDef =>
        trackedTrees.update(member.symbol, tree)
        tree
      case _ => tree
  end updateTracked
  private def withUpdatedTrackedTrees(stats: List[Tree]) =
    val trackedTrees = currentTrees
    stats.mapConserve:
      case tree: MemberDef if trackedTrees.contains(tree.symbol) =>
        trackedTrees(tree.symbol)
      case stat => stat
  def transform(tree: Tree): Tree =
    tree match
      case TypeDef(_, impl: Template) =>
        inContext(trackedDefinitionsCtx(impl.body)):
          val newTree = super.transform(tree)
          newTree
      case _ => super.transform(tree)
end Outer

object Following
