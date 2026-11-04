package adt

sealed trait Expr

final case class Num(value: Int) extends Expr
final case class Add(left: Expr, right: Expr) extends Expr

object Foreign:
  object Add:
    def unapply(value: Expr): Option[(Expr, Expr)] = Some((Num(0), Num(0)))

object Evaluator:
  def peelLeft(expr: Expr): Expr =
    expr match
      case Foreign.Add(Add(left, _), _) => left
      case _ => expr

  def sum(expr: Expr): Int =
    expr match
      case Num(value) => value
      case Foreign.Add(Num(value), right) =>
        value + sum(right)
      case Foreign.Add(left, right) =>
        sum(left) + sum(right)
      case _ => 0
