class UserDefinedAnd {
  def &&(other: Boolean): Boolean = other
  def call(other: Boolean): Boolean = this && other
}
