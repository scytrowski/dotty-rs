package adtcontrol

sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Succ(inner: Expr) extends Expr

object Control:
  def partial(expr: Expr): Int = expr match
    case Num(value) => value
