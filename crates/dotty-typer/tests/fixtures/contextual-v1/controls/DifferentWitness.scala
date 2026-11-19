trait Ev[A] { type Out }
class C {
  def read[A: Ev as tc](v: tc.Out)(using n: Int = 1): Int = n
  def wrong(e1: Ev[Int], e2: Ev[Int])(v: e2.Out): Int = read[Int](using e1)(v)
}
