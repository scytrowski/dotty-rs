package adt

sealed trait Expr

final case class Num(value: Int) extends Expr
final case class Add(left: Expr, right: Expr) extends Expr

object Evaluator:
  def peelLeft(expr: Expr): Expr =
    expr match
      case Add(Add(left, _), _) => left
      case _ => expr

  def sum(expr: Expr): Int =
    expr match
      case Num(value) => value
      case Add(Num(value, extra), right) =>
        value + sum(right)
      case Add(left, right) =>
        sum(left) + sum(right)
