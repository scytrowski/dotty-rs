package exportsprint
class TC[A]
object Instances:
  given ints: TC[Int] = new TC[Int]
  given flags: TC[Boolean] = new TC[Boolean]
  private given hidden: TC[Long] = new TC[Long]
object Named:
  export Instances.ints
object Alias:
  export Instances.{ints as selected}
object All:
  export Instances.given
object Other:
  given another: TC[Int] = new TC[Int]
