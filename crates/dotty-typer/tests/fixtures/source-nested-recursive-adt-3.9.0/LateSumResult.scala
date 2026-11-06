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
      case Add(Num(value), right) =>
        value + sum(right)
      case Add(left, right) =>
        sum(left) + sum(right)

object LateSumControl:
  def sumFails(expr: Expr): Boolean =
    expr match
      case Num(_) => true
      case Add(Num(value), right) => value + Evaluator.sum(right)
      case Add(left, right) => Evaluator.sum(left) + Evaluator.sum(right)
