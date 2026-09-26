{
  def method[A: Show](value: A): A = value
  def aliased[A: Show as show](value: A): A = value
  def multiple[A: {Ord, Show}]: Int = 1
  def bounded[A >: Low <: High: Show]: Int = 1
  class C[A: Show]
  case class D[A: Show](value: A)
  given [A: Show] => Evidence[A] = evidence
  extension [A: Show](value: A) def get: A = value
}
