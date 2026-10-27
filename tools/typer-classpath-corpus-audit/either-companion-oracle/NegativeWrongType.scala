package issue1747oracle

object NegativeWrongType:
  case class Node[A](value: A)
  def apply = Node[String](1)
