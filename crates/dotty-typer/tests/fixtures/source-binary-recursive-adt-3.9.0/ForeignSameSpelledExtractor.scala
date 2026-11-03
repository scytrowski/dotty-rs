package adt

sealed trait Expr

final case class Num(value: Int) extends Expr
final case class Add(left: Expr, right: Expr) extends Expr

object Foreign:
  object Add:
    def unapply(value: Expr): adt.Add = adt.Add(Num(0), Num(0))

object Evaluator:

  def selectLeft(expr: Expr): Expr =
    expr match
      case Num(_)       => expr
      case Foreign.Add(left, _) => left

  def eval(expr: Expr): Int =
    expr match
      case Num(value)       => value
      case Foreign.Add(left, right) => eval(left) + eval(right)
