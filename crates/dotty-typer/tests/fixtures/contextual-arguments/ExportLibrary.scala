package exportsprint
class TC[A]
class Box[A]
object Instances:
  given ints: TC[Int] = new TC[Int]
  given flags: TC[Boolean] = new TC[Boolean]
  given boxed[A](using e: TC[A]): TC[Box[A]] = new TC[Box[A]]
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
object Conditional:
  export Instances.boxed
