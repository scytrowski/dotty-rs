package inheritexport
class TC[A]
trait Base[A]:
  given key: TC[A] = new TC[A]
object Ints extends Base[Int]
object Api:
  export Ints.given
object Named:
  export Ints.{key as selected}
object Rival:
  given other: TC[Int] = new TC[Int]
trait Low:
  given low: TC[Int] = new TC[Int]
object High extends Low:
  given high: TC[Int] = new TC[Int]
object Ranked:
  export High.given
class Good:
  def need[A](using e: TC[A]): TC[A] = e
  def direct: TC[Int] = Ints.key
  def directSearch: TC[Int] = { import Ints.given; need[Int] }
  def named: TC[Int] = Named.selected
  def all: TC[Int] = { import Api.given; need[Int] }
  def rankedControl: TC[Int] = { import High.given; need[Int] }
