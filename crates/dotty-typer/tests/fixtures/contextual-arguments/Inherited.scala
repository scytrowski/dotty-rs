class Broad
class Narrow extends Broad
trait TC[A, +B]
class Parent[A] extends TC[A, Narrow]
class Impl[A] extends Parent[A]
object TC {
  given instance: [A] => Impl[A] = new Impl[A]
}
class Inherited {
  def need(using n: TC[Int, Broad]): Int = 1
  def run: Int = need
}
