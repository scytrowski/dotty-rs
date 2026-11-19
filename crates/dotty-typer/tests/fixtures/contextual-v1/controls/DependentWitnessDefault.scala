trait Ev[A] { type Out }
class tc { type Out }
class C {
  def read[A: Ev as tc](v: tc.Out)(using n: Int = 1): Int = n
  def shadow[tc: Ev as tc](v: tc.Out)(using n: Int = 1): Int = n
  def project[A: Ev as tc](v: tc#Out)(using n: Int = 1): Int = n
  def explicit(e: Ev[Int])(v: e.Out): Int = read[Int](using e)(v)(using 1)
  def fallback(e: Ev[Int])(v: e.Out): Int = read[Int](using e)(v)
}
