package legacymembers

class Rejected {
  def privateMember(x: Box[Int]): Int = x.hidden
  def absent(x: Box[Int]): Int = x.absent
}

class Ambiguous {
  implicit def first[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
  implicit def second[A](x: Box[A]): Wrapped[A] = new Wrapped[A]
  def ambiguous(x: Box[Int]): Int = x.show
}
