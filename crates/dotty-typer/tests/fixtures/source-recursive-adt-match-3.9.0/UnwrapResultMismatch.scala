package adtcontrol

sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Succ(inner: Expr) extends Expr

object Control:
  def unwrap(expr: Expr): String = expr match
    case Num(_)      => expr
    case Succ(inner) => inner
