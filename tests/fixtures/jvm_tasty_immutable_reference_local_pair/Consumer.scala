package example

object Consumer:
  private def field(pair: Pair, name: String): Counter =
    val value = classOf[Pair].getDeclaredField(name)
    value.setAccessible(true)
    value.get(pair).asInstanceOf[Counter]

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    val existing = new Counter(7)
    val primary = factory.create(existing, 3)
    val primaryLeft = field(primary, "left")
    val primaryRight = field(primary, "right")
    assert(primaryLeft eq existing)
    assert(!(primaryRight eq existing))
    assert(primaryLeft.get() == 7 && primaryRight.get() == 3)
    primaryLeft.set(8)
    assert(primaryLeft.get() == 8 && primaryRight.get() == 3)
    primaryRight.set(9)
    assert(primaryLeft.get() == 8 && primaryRight.get() == 9)
    val primaryAgain = field(factory.create(existing, 3), "right")
    assert(!(primaryAgain eq primaryRight))

    println("scala39-immutable-reference-local-pair=ok")
