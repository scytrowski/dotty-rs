package inheritexport
class TC[A]
trait Base[A]:
  given key: TC[A] = new TC[A]
object Ints extends Base[Int]
object Named:
  export Ints.{key as selected}
class Good:
  def need[A](using e: TC[A]): TC[A] = e
  def direct: TC[Int] = Ints.key
  def directSearch: TC[Int] = { import Ints.given; need[Int] }
  def named: TC[Int] = Named.selected
