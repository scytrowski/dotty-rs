{
  class Box[A](x: A):
    val value = x
    def get: A = value
    type Value = A
}
