object ScalaConsumer {
  def main(args: Array[String]): Unit = {
    val value = new example.Expressions()
    val inputs = Array(Int.MinValue, -257, -1, 0, 1, 2, 255, 256, 257, Int.MaxValue)
    for x <- inputs do {
      assert(value.intValue(x) == x)
      assert(value.mask(x) == (x & 255))
      assert(value.classify(x) == (if x == 0 then 10 else if x == 1 then 20 else 30))
      assert(value.flag(x) == (x != 0))
      val expected = if (x & 255) == 0 then 10 else if (x & 255) == 1 then (if x == 1 then 20 else 30) else 40
      assert(value.composed(x) == expected)
      for y <- inputs do {
        assert(value.equal(x, y) == (x == y))
        assert(value.different(x, y) == (x != y))
      }
    }
    assert(!value.booleanValue(false) && value.booleanValue(true))
    value.unitValue()
    assert(value.read() == 0)
    value.touch(0)
    value.touch(5)
    assert(value.read() == 3)
    val selector = new example.Expressions()
    assert(selector.once() == 11 && selector.read() == 1)
    assert(selector.once() == 22 && selector.read() == 2)
    println("expression parity ok")
  }
}
