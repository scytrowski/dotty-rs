object ScalaConsumer {
  def main(args: Array[String]): Unit = {
    val v = new example.EarlyReturns
    assert(v.direct(-1) == -1)
    assert(v.booleanValue(true))
    v.unit()
    assert(v.both(3) == 4)
    assert(v.matched(-3) == -3)
    assert(v.matched(3) == 4)
    assert(v.inLoop(5) == 3)
    assert(v.choose(-1) == 1)
    assert(v.observed() == 1)
    println("local return oracle ok")
  }
}
