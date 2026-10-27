package issue1747oracle

object PrivateOwner:
  case class Secret private (value: Int)

object NegativePrivateConstructor:
  def apply = PrivateOwner.Secret(1)
