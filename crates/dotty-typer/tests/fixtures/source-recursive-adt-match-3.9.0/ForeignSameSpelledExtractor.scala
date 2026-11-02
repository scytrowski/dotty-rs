package adtcontrol

sealed trait Expr
final case class Num(value: Int) extends Expr
final case class Succ(inner: Expr) extends Expr

object Foreign:
  object Succ:
    def unapply(value: String): Some[String] = Some(value)

object Control:
  def unwrap(expr: Expr): Expr =
    val Succ = Foreign.Succ
    expr match
      case Succ(inner) => inner
      case Num(_)      => expr
