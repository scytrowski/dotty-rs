package adtcontrol

sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Succ(inner: String) extends Expr

object Control:
  def component(expr: Expr): String = expr match
    case Succ(value) => value
    case Num(_)      => "num"
