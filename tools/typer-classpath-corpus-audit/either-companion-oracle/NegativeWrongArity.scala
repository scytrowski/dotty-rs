package issue1747oracle

object NegativeWrongArity:
  case class Node[A](value: A)
  def apply[A] = Node()
