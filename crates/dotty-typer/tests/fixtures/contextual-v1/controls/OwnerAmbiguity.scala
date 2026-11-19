class A
class B
object Cases {
  given a1: A = ???
  given a2: A = ???
  def g(using a: A): B = ???
  def f: Unit = g
}
