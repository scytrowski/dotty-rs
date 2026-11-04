package example

object Consumer:
  private def field(pair: Pair, name: String): Counter =
    val value = classOf[Pair].getDeclaredField(name)
    value.setAccessible(true)
    value.get(pair).asInstanceOf[Counter]

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    val existing = new Counter(7)
    val pair = factory.createReversed(existing, 3)
    val left = field(pair, "left")
    val right = field(pair, "right")
    assert(right eq existing)
    assert(!(left eq existing))
    assert(left.get() == 3 && right.get() == 7)
    assert(pair.leftValue() == 3 && pair.rightValue() == 7)
    existing.set(42)
    assert(left.get() == 3 && right.get() == 42)
    assert(pair.leftValue() == 3 && pair.rightValue() == 42)
    left.set(-9)
    assert(left.get() == -9 && right.get() == 42)
    assert(pair.leftValue() == -9 && pair.rightValue() == 42)

    val eleven = field(factory.createReversed(existing, 11), "left")
    val zero = field(factory.createReversed(existing, 0), "left")
    val negative = field(factory.createReversed(existing, -5), "left")
    assert(!(eleven eq left) && !(zero eq left) && !(negative eq left))
    assert(!(eleven eq zero) && !(eleven eq negative) && !(zero eq negative))
    assert(eleven.get() == 11 && zero.get() == 0 && negative.get() == -5)
    assert(field(factory.createReversed(existing, 11), "right") eq existing)

    println("scala39-immutable-reference-local-pair-reversed=ok; identity=true; fresh=true; mutation-isolated=true; seeds=3,11,0,-5")
