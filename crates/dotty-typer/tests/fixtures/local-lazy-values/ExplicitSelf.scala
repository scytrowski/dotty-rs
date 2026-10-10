class ExplicitSelfRecursiveLazyValue {
  def explicitSelf: Int = {
    lazy val value: Int = value
    value
  }
}
