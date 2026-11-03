package example

object Consumer:
  private def field(pair: Pair, name: String): Counter =
    val value = classOf[Pair].getDeclaredField(name)
    value.setAccessible(true)
    value.get(pair).asInstanceOf[Counter]

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    val existing = new Counter(7)
    val reversed = factory.createReversed(existing, 3)
    val reversedLeft = field(reversed, "left")
    val reversedRight = field(reversed, "right")
    assert(reversedRight eq existing)
    assert(!(reversedLeft eq existing))
    assert(reversedLeft.get() == 3 && reversedRight.get() == 7)
    reversedRight.set(8)
    assert(reversedLeft.get() == 3 && reversedRight.get() == 8)
    reversedLeft.set(9)
    assert(reversedLeft.get() == 9 && reversedRight.get() == 8)
    val reversedAgain = field(factory.createReversed(existing, 3), "left")
    assert(!(reversedAgain eq reversedLeft))
    println("scala39-immutable-reference-local-pair-reversed=ok")
