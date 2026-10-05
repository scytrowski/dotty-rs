object PatDefBinderVisibilityOracle:
  def visibleAfterDefinition(value: Option[Int]): Int =
    val Some(x) = value
    x

  def forwardReference(): Int =
    val Some(x) = Some(later)
    val later = 1
    x
