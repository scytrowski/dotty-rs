package adtcontrol

sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Succ(inner: Expr) extends Expr
final case class Other(value: String)

object Control:
  def unrelated(expr: Expr): String = expr match
    case Other(value) => value
