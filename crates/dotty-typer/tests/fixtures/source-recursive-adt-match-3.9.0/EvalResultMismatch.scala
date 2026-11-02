package adtcontrol

sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Succ(inner: Expr) extends Expr

object Control:
  def eval(expr: Expr): Expr = expr match
    case Num(value)  => value
    case Succ(inner) => eval(inner) + 1
