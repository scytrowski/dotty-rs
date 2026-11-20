trait TC[A]
trait Trace
case class Envelope[A: TC](value: A)(using trace: Trace)

class Client {
  def allocation(using tc: TC[Int], log: Trace): Envelope[Int] = new Envelope[Int](1)
  def build(using tc: TC[Int], log: Trace): Envelope[Int] = Envelope(1)
  def explicit(tc: TC[Int], log: Trace): Envelope[Int] = new Envelope[Int](1)(using tc, log)
  def duplicate(initial: TC[Int], previous: Trace)(using current: TC[Int], active: Trace): Envelope[Int] = {
    val original: Envelope[Int] = Envelope(1)(using initial, previous)
    original.copy()
  }
}
