object MemberLookupOracle {
  def short2float(x: Short): Float = x.toFloat
  def byte2double(x: Byte): Double = x.toDouble
  def char2double(x: Char): Double = x.toDouble
  def doubleNegate(x: Double): Double = -x
  def numericDoubleNegate: Double = scala.math.Numeric.DoubleIsFractional.negate(1.0d)

  final class SourceNumber {
    def unary_- : SourceNumber = this
  }
  def sourceOwnedNegate(x: SourceNumber): SourceNumber = -x
}
