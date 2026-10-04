class F[A, B, C, D, E, G]
class Foo
class Bar
class Container[X]
object Outer:
  class Bound

class WildcardTypes:
  def wildcard(
      value: F[
        ?,
        ? <: Foo,
        ? >: Bar,
        ? >: Bar <: Foo,
        ? <: Container[Outer.Bound],
        ? >: Outer.Bound
      ]
  ): Unit = ()
