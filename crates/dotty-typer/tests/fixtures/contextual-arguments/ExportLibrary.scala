package exportsprint
class TC[A]
object Instances:
  given ints: TC[Int] = new TC[Int]
  given flags: TC[Boolean] = new TC[Boolean]
  private given hidden: TC[Long] = new TC[Long]
object Legacy:
  implicit val old: TC[Int] = new TC[Int]
object Named:
  export Instances.ints
object Alias:
  export Instances.{ints as selected}
object All:
  export Instances.given
object Only:
  export Instances.given TC[Int]
object Star:
  export Instances.*
object Old:
  export Legacy.*
object Other:
  given another: TC[Int] = new TC[Int]
