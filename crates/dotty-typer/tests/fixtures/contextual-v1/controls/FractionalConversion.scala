package fractionalprobe
import scala.math.Fractional

object FractionalConversion:
  def explicit[T](x: T, y: T)(using f: Fractional[T]): T =
    Fractional.Implicits.infixFractionalOps[T](x)(using f)./(y)
  def searched[T](x: T, y: T)(using f: Fractional[T]): T =
    Fractional.Implicits.infixFractionalOps[T](x)./(y)
  def inferred[T](x: T, y: T)(using f: Fractional[T]): T =
    Fractional.Implicits.infixFractionalOps(x)./(y)
