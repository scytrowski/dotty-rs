package structural

sealed trait Expr
final case class Add(left: Expr, right: Expr) extends Expr
case class Pair(first: Expr, second: Expr)
case class Reordered(second: Expr, first: Expr)
case class SameType(first: String, second: String)
