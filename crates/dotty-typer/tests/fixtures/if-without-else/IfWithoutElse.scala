object IfWithoutElse {
  def inferred(flag: Boolean) = if (flag) "value"

  def statement(flag: Boolean): Unit = {
    if (flag) "value"
  }

  def incompleteChain(flag: Boolean, innerFlag: Boolean) =
    if (flag) "outer" else if (innerFlag) "inner"

  def explicitUnit(flag: Boolean) = if (flag) "value" else ()

  val inferredResult: Unit = inferred(true)
  val incompleteChainResult: Unit = incompleteChain(true, false)
}
