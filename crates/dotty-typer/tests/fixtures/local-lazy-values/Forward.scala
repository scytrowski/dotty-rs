class ForwardLocalLazyValue {
  def explicitForward: Int = {
    lazy val first: Int = later
    lazy val later: Int = 1
    first
  }
}
