package adt

sealed trait Expr

final case class Num(value: Int) extends Expr
final case class Other(value: Int)
final class OtherPair(val _1: Other, val _2: Other):
  def isEmpty: Boolean = false
  def get: OtherPair = this

final class Add(val left: Other, val right: Other) extends Expr

object Add:
  def unapply(value: Add): OtherPair = new OtherPair(value.left, value.right)

object Evaluator:
  def selectLeft(expr: Expr): Expr =
    expr match
      case Num(_)       => expr
      case Add(_, _)    => expr

  def eval(expr: Expr): Int =
    expr match
      case Num(value)       => value
      case Add(left, right) => eval(left) + eval(right)
