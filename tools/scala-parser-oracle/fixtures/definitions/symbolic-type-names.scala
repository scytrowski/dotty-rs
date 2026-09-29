{
  type ::[+A] = List[A]
  type =:=[A, B] = (A, B)

  val cons: ::[Int] = ???
  val equality: =:=[Int, Int] = ???

  def fromCons[T](xs: ::[T]): ::[T] = xs
  def evidence[A](eq: =:=[A, A]): A = ???

  val constructed = new ::[Int](1, Nil)
}
