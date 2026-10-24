class IncompatibleReference {}
class IncompatibleBitwiseAnd {
  def invalid(value: IncompatibleReference): Int = value & 1
}
