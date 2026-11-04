package adt

sealed trait Expr

final case class Num(value: Int) extends Expr
final case class Other(value: Int) extends Expr
final case class Add(left: Other, right: Other) extends Expr

object Evaluator:
  def selectLeft(expr: Expr): Expr =
    expr match
      case Num(_)       => expr
      case Add(left, _) => left

  def eval(expr: Expr): Int =
    expr match
      case Num(value)       => value
      case Add(left, right) => eval(left) + eval(right)
