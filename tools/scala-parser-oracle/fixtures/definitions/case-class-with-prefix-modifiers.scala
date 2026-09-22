{
  @Ann
  final case class Box[A](value: A, count: Int) extends Parent(value):
    def get: A = value
}
