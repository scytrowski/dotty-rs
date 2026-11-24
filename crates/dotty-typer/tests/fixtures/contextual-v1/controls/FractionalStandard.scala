import scala.math.Fractional

object FractionalStandard:
  def standard: Fractional[Double] = summon[Fractional[Double]]
  def single: Fractional[Float] = summon[Fractional[Float]]
