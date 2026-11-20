trait TC[A]
trait Trace
case class Envelope[A: TC](value: A)(using trace: Trace)

class Client {
  def newTc(using other: Trace): Envelope[Int] = new Envelope[Int](1)
  def applyTc(using other: Trace): Envelope[Int] = Envelope(1)
  def copyTc(e: Envelope[Int])(using other: Trace): Envelope[Int] = e.copy()
  def newTrace(using other: TC[Int]): Envelope[Int] = new Envelope[Int](1)
  def applyTrace(using other: TC[Int]): Envelope[Int] = Envelope(1)
  def copyTrace(e: Envelope[Int])(using other: TC[Int]): Envelope[Int] = e.copy()
}
