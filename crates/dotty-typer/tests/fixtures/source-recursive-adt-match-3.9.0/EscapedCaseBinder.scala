package adtcontrol

sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Succ(inner: Expr) extends Expr

object Control:
  def unwrap(expr: Expr): Expr = expr match
    case Succ(inner) => inner
    case Num(_)      => expr
  val escaped: Expr = inner
