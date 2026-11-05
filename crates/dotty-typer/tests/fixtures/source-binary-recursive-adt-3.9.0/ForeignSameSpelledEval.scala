package adt

sealed trait Expr

final case class Num(value: Int) extends Expr
final case class Add(left: Expr, right: Expr) extends Expr

object Helper:
  def eval(expr: Expr): Int = 0

object Evaluator:
  def selectLeft(expr: Expr): Expr =
    expr match
      case Num(_)       => expr
      case Add(left, _) => left

  def eval(expr: Expr): Int =
    expr match
      case Num(value)       => value
      case Add(left, right) => Helper.eval(left) + Helper.eval(right)
