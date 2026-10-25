class ForwardLocalLazyValue {
  def explicitForward: Int = {
    lazy val first: Int = later
    lazy val later: Int = 1
    first
  }

  def inferredForward: Int = {
    lazy val first: Int = later
    lazy val later = 1
    first
  }
}
