package adtcontrol

sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Succ(inner: Expr) extends Expr

object Control:
  def unwrap(expr: Expr): Expr = expr match
    case Succ(left, right) => left
    case Num(value)        => Num(value)
