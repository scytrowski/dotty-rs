package issue1747oracle

object AmbiguousOverload:
  case class Node[A](value: A)
  object Node:
    def apply(value: String): Node[String] = new Node(value)
    def apply(value: java.lang.Integer): Node[Integer] = new Node(value)
  def apply = Node(null)
