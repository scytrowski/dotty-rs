object NestedMatchInfixContinuation {
  def run(y: Any) =
      try (this eq y)
      || maxSubsumes(y, canAddHidden = false)
      || y.match
        case y: TermRef =>
            y.prefix.match
              case ypre: Capability =>
                this.subsumes(ypre)
                || this.match
                    case x @ TermRef(xpre: Capability, _) if x.symbol == y.symbol =>
                      withMode(Mode.IgnoreCaptures):
                        TypeComparer.isSameRef(xpre, ypre)
                    case _ => false
              case _ => false
          || viaInfo(y.info)(subsumingRefs(this, _))
        case Maybe(y1) => this.stripMaybe.subsumes(y1)
        case _ => false
      || this.match
          case Reach(x1) => x1.subsumes(y.stripReach)
          case _ => false
      catch case _ => false
  def following = true
}
