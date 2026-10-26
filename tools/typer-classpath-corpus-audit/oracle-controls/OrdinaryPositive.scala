object OrdinaryOracleLeftOwner {
  def sameName: Int = 1
}

object OrdinaryOracleRightOwner {
  def sameName: String = "right"
}

object OrdinaryOraclePositive {
  def identity[A](value: A): A = value

  def nestedShadow(value: Int): Int = {
    def inner(value: Int): Int = value
    inner(value)
  }

  def leftOwner: Int = OrdinaryOracleLeftOwner.sameName
  def rightOwner: String = OrdinaryOracleRightOwner.sameName
  val identityControl: Int = identity(1)
}
