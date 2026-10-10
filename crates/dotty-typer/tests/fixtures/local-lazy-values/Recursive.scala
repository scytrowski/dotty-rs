class RecursiveLocalLazyValues {
  def inferredSelf: Int = {
    lazy val value = value
    value
  }

  def inferredMutual: Int = {
    lazy val first = later
    lazy val later = first
    first
  }

}
