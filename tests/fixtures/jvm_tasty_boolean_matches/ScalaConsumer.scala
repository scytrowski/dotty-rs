object ScalaConsumer {
  def main(args: Array[String]): Unit = {
    val p = new example.BooleanMatches()
    for (x <- List(false, true); y <- List(false, true)) {
      assert(p.bind(x) == x)
      assert(p.inverted(x) == !x)
      assert(p.conjunction(x, y) == (x && y))
      assert(p.loop(3, x) == (if x then 3 else 0))
      assert(p.shadow(x, y) == x)
      assert(p.nested(x, y) == x)
      assert(p.shortCircuit(x, y) == (if x && y then 1 else 2))
      assert(p.longValue(x, Long.MinValue, Long.MaxValue) == (if x then Long.MinValue else Long.MaxValue))
      assert(java.lang.Double.doubleToRawLongBits(p.returned(x, -0.0, java.lang.Double.MIN_VALUE)) == (if x then Long.MinValue else 1L))
    }
    assert(p.singleton())
    assert(!p.falseSingleton())
    println("boolean match oracle ok")
  }
}
