package inheritexport
class TC[A]
class Pair[A,B]
trait Base[A]:
  given key: TC[A] = new TC[A]
  given pair[B](using e: TC[B]): TC[Pair[A,B]] = new TC[Pair[A,B]]
  private given hidden: TC[Long] = new TC[Long]
object Ints extends Base[Int]
object Bools extends Base[Boolean]
object Api:
  export Ints.given
object Named:
  export Ints.{key as selected}
object Only:
  export Ints.given TC[Int]
object Second:
  export Api.given
object Rival:
  given other: TC[Int] = new TC[Int]
trait OldBase:
  implicit val old: TC[Int] = new TC[Int]
trait LeftSide extends OldBase
trait RightSide extends OldBase
object Diamond extends LeftSide with RightSide
object Both:
  export Diamond.*
object HiddenByOverride extends OldBase:
  override val old: TC[Int] = new TC[Int]
object OldApi:
  export HiddenByOverride.*
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
  def poly: TC[Pair[Int,Int]] = { import Api.given; need[Pair[Int,Int]] }
  def chain: TC[Int] = { import Second.given; need[Int] }
  def diamond: TC[Int] = { import Both.*; need[Int] }
  def rankedControl: TC[Int] = { import High.given; need[Int] }
object Shared extends Base[Int]:
  export Ints.given
class SharedClient:
  def need[A](using e: TC[A]): TC[A] = e
  def use: TC[Int] = { import Shared.given; need[Int] }
