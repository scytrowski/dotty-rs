object UniversalEquality {
  final class Box

  def intLiteral(value: Int): Boolean = value == 1
  def any(left: Any, right: Any): Boolean = left == right
  def references(left: String, right: String): Boolean = left != right
  def custom(left: Box, right: Box): Boolean = left == right
  def booleans(left: Boolean, right: Boolean): Boolean = left == right
}
