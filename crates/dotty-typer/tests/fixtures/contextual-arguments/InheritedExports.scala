package inheritexport
class TC[A]
trait Base[A]:
  given key: TC[A] = new TC[A]
  private given hidden: TC[Long] = new TC[Long]
object Ints extends Base[Int]
object Bools extends Base[Boolean]
object Api:
  export Ints.given
object Named:
  export Ints.{key as selected}
object Only:
  export Ints.given TC[Int]
object Rival:
  given other: TC[Int] = new TC[Int]
trait Low:
  given low: TC[Int] = new TC[Int]
object High extends Low:
  given high: TC[Int] = new TC[Int]
object Ranked:
  export High.given
object BoolOnly:
  export Bools.given TC[Boolean]
class Good:
  def need[A](using e: TC[A]): TC[A] = e
  def direct: TC[Int] = Ints.key
  def directSearch: TC[Int] = { import Ints.given; need[Int] }
  def named: TC[Int] = Named.selected
  def all: TC[Int] = { import Api.given; need[Int] }
  def typed: TC[Int] = { import Only.given; need[Int] }
  def bool: TC[Boolean] = { import BoolOnly.given; need[Boolean] }
  def rankedControl: TC[Int] = { import High.given; need[Int] }
