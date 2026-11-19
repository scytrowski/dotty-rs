package growth

class F[A]
class Box[A]

class Growing {
  implicit def grow[A](implicit more: => F[Box[A]]): F[A] = new F[A]
  implicit val growingLeaf: F[Box[Box[Int]]] = new F[Box[Box[Int]]]
  def ask(using f: F[Int]): Int = 1
  def growing: Int = ask
}

class Shrinking {
  implicit def shrink[A](implicit less: => F[A]): F[Box[A]] = new F[Box[A]]
  implicit val shrinkingLeaf: F[Int] = new F[Int]
  def ask(using f: F[Box[Box[Int]]]): Int = 1
  def shrinking: Int = ask
}
