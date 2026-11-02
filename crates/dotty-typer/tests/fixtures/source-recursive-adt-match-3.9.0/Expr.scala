package adt

sealed trait Expr

final case class Num(value: Int) extends Expr
final case class Succ(inner: Expr) extends Expr

object Evaluator:
  def unwrap(expr: Expr): Expr =
    expr match
      case Num(_)      => expr
      case Succ(inner) => inner

  def eval(expr: Expr): Int =
    expr match
      case Num(value)  => value
      case Succ(inner) => eval(inner) + 1
