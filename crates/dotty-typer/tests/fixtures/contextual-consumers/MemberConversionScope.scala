package memberscope

class From
class To { def show: Int = 2 }
class Hidden { private def show: Int = 0 }
class Other
class Mimic { def apply(x: From): To = new To }
object From {
  given render: Conversion[From, To] with {
    def apply(x: From): To = new To
  }
}
class Client {
  def fromCompanion(x: From)(using irrelevant: Conversion[Other, To], hidden: Conversion[From, Hidden], numeric: Conversion[From, Int], callable: Mimic): Int = x.show
  def fromScope[A](x: From)(using unknown: Conversion[From, A], chosen: Conversion[From, To]): Int = x.show
}
