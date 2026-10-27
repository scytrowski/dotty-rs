package issue1747oracle

sealed abstract class EitherLike[+A, +B]:
  def left: EitherLike.LeftProjection[A, B] = EitherLike.LeftProjection(this)
  def right: EitherLike.RightProjection[A, B] = EitherLike.RightProjection(this)

object EitherLike:
  final case class LeftProjection[+A, +B](e: EitherLike[A, B])
  final case class RightProjection[+A, +B](e: EitherLike[A, B])

object Owners:
  object First:
    case class Node[A](value: A)
  object Second:
    case class Node[A](value: A)
  def first: First.Node[Int] = First.Node(1)
  def second: Second.Node[String] = Second.Node("x")

object UserCompanion:
  case class Node[A](value: A)
  object Node:
    def apply(value: String, tag: Int): Node[String] = new Node(value)
  def generatedApply: Node[Int] = Node(1)
  def overloadedApply: Node[String] = Node("x", 1)

object Ordinary:
  final class Node[A](value: A)
  def constructor: Node[Int] = Node(1)
