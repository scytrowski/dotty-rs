object NestedCaseClassShapes:
  case class Zero()
  case class One(value: Int)
  case class Defaulted(value: Int = 1)
  case class Curried(value: Int)(tag: Int)
  case class Contextual(value: Int)(using ordering: Ordering[Int])
  case class Repeated(values: Int*)
  case class PrivateConstructor private (value: Int)
  case class Secondary(value: Int):
    def this() = this(0)
  case class Bounded[A <: AnyRef](value: A)
  case class ContextBound[A: Ordering](value: A)
  abstract case class Abstract(value: Int)
  case class Explicit(value: Int = 1)
  object Explicit:
    def apply(value: String): Explicit = new Explicit(1)
  def unrelated: Int = 1
