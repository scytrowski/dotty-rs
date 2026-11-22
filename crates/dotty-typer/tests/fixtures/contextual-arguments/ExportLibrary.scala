package exportsprint
class TC[A]
object Instances:
  given ints: TC[Int] = new TC[Int]
object Named:
  export Instances.ints
object Alias:
  export Instances.{ints as selected}
