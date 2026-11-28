object ScalaConsumer {
  var checks = 0
  def check(actual: Boolean, expected: Boolean): Unit = { assert(actual == expected); checks += 1 }
  def check(actual: Int, expected: Int): Unit = { assert(actual == expected); checks += 1 }
  def throwsArithmetic(f: => Unit): Unit = {
    var thrown = false
    try f catch { case _: ArithmeticException => thrown = true }
    assert(thrown)
    checks += 1
  }
  def main(args: Array[String]): Unit = {
    val p = new example.BooleanCalls()
    check(p.noarg(), true)
    for (x <- List(-2, 0, 3)) {
      val positive = x > 0
      check(p.direct(x), positive)
      check(p.explicit(x), positive)
      check(p.nested(x), positive)
      check(p.not(x), !positive)
      check(p.local(x), positive)
      check(p.returns(x), positive)
      check(p.condition(x), if (positive) 1 else 2)
      check(p.selector(x), if (positive) 1 else 2)
      check(p.guard(x), if (positive) 1 else 2)
      check(p.intCall(x), if (positive) 1 else 2)
      check(p.loop(x), if (x > 0) 0 else x)
      for (b <- List(false, true)) {
        check(p.mixed(x, b), b && positive)
        check(p.mixedCall(x, b), b && positive)
      }
    }
    for (a <- List(false, true)) {
      for (b <- List(false, true)) {
        check(p.both(a, b), a && b)
        check(p.ordered(a, b), 12)
        check(p.eagerState(a, b), 12)
        check(p.andState(a, b), if (a) 12 else 1)
        check(p.orState(a, b), if (a) 1 else 12)
        check(p.conditionalArg(a, b), if (a) b else !b)
      }
      throwsArithmetic(p.eager(a, 0))
      if (a) throwsArithmetic(p.shortAnd(a, 0)) else check(p.shortAnd(a, 0), false)
      if (!a) throwsArithmetic(p.shortOr(a, 0)) else check(p.shortOr(a, 0), true)
    }
    throwsArithmetic(p.discard(0))
    p.discard(2); checks += 1
    println(s"OK $checks checks")
  }
}
