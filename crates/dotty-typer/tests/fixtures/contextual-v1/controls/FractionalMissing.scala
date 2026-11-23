import scala.math.Fractional

object FractionalMissing:
  def missing[T]: Fractional[T] = summon[Fractional[T]]
