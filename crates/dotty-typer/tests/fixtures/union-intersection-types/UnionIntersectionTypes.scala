class A
class B
class C
class F[T]

object Outer:
  class Nested

object UnionIntersectionTypes:
  type Alias = Outer.Nested & A

  val simpleUnion: A | B = ???
  val simpleIntersection: A & B = ???
  val precedence: A | B & C = ???
  val parenthesized: (A | B) & C = ???
  val applied: F[A | B] = ???
  val qualified: Outer.Nested | A = ???

  def signature(value: A | B): A & C = ???
