package structural

class Box[A]
trait TC[A]:
  def tag: Int

object Builders:
  given boxes: [A] => (base: TC[A]) => TC[Box[A]]:
    def tag: Int = base.tag

class Client:
  import Builders.given
  def need(using t: TC[Box[Int]]): Int = t.tag
  def inferred(using leaf: TC[Int]): Int = need
  def explicit(using leaf: TC[Int]): Int = Builders.boxes[Int](using leaf).tag
