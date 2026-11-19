package evidence
class Box[A]
class TC[A]
object TC:
  given ints: TC[Int] = new TC[Int]
  given boxes: [A] => TC[A] => TC[Box[A]] = new TC[Box[A]]
