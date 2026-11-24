package fractionalprobe
import scala.math.Fractional

object Errors:
  def missing[T](x: T, y: T): T =
    Fractional.Implicits.infixFractionalOps(x)./(y)
  def ambiguous[T](x: T, y: T)(using a: Fractional[T], b: Fractional[T]): T =
    Fractional.Implicits.infixFractionalOps(x)./(y)
