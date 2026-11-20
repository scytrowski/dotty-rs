package bound
trait TC[A] { def tag:Int }
class Box[A]
object Builders {
  given boxes: [A: TC as base] => TC[Box[A]]:
    def tag:Int = base.tag
}
class Client {
  import Builders.given
  def need(using t:TC[Box[Int]]):Int=t.tag
  def inferred(using leaf:TC[Int]):Int=need
}
