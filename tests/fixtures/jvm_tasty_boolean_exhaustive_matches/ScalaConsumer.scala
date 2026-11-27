object ScalaConsumer {
  def main(args: Array[String]): Unit = {
    val p = new example.BooleanExhaustiveMatches()
    for (x <- List(false, true)) {
      assert(p.ordered(x) == (if x then 1 else 2))
      assert(p.bind_literals(x) == x)
      assert(p.guard_effect(x) == (if x then 11 else 20))
      assert(p.selector_effect(x) == (if x then 11 else 21))
      assert(p.result_long(x) == (if x then Long.MaxValue else Long.MinValue))
      assert(java.lang.Float.floatToRawIntBits(p.result_float(x)) == java.lang.Float.floatToRawIntBits(if x then -0.0f else java.lang.Float.MIN_VALUE))
      assert(java.lang.Double.doubleToRawLongBits(p.result_double(x)) == java.lang.Double.doubleToRawLongBits(if x then -0.0 else java.lang.Double.MIN_VALUE))
      for (y <- List(false, true)) {
        assert(p.return_boolean(x, y, !y) == (if x then y else !y))
        assert(p.nested(x, y) == (if x then (if y then 1 else 2) else 3))
      }
    }
    println("exhaustive boolean match oracle ok")
  }
}
