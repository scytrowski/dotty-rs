object MatchCaseGuardInfixPattern:
  def extract(tree: Tree): Option[Tree] = tree match
    case Apply(fn, result :: Nil)
    if fn.symbol.is(Implicit)
    || fn.symbol.name == nme.apply && fn.symbol.owner.derivesFrom(defn.ConversionClass)
    => Some(result)
    case _ => None

  def following: Int = 1
