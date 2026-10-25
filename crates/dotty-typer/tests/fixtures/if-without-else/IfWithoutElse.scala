object IfWithoutElse {
  def inferred(flag: Boolean) = if (flag) "value"

  def statement(flag: Boolean): Unit = {
    if (flag) "value"
  }

  def explicitUnit(flag: Boolean) = if (flag) "value" else ()

  val inferredResult: Unit = inferred(true)
}
