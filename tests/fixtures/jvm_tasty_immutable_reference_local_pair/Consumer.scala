package example

object Consumer:
  private def field(pair: Pair, name: String): Counter =
    val value = classOf[Pair].getDeclaredField(name)
    value.setAccessible(true)
    value.get(pair).asInstanceOf[Counter]

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    val existing = new Counter(7)
    val pair = factory.create(existing, 3)
    val left = field(pair, "left")
    val right = field(pair, "right")
    assert(left eq existing)
    assert(!(right eq existing))
    assert(left.get() == 7 && right.get() == 3)
    assert(pair.leftValue() == 7 && pair.rightValue() == 3)
    existing.set(42)
    assert(left.get() == 42 && right.get() == 3)
    assert(pair.leftValue() == 42 && pair.rightValue() == 3)
    right.set(-9)
    assert(left.get() == 42 && right.get() == -9)
    assert(pair.leftValue() == 42 && pair.rightValue() == -9)

    val eleven = field(factory.create(existing, 11), "right")
    val zero = field(factory.create(existing, 0), "right")
    val negative = field(factory.create(existing, -5), "right")
    assert(!(eleven eq right) && !(zero eq right) && !(negative eq right))
    assert(!(eleven eq zero) && !(eleven eq negative) && !(zero eq negative))
    assert(eleven.get() == 11 && zero.get() == 0 && negative.get() == -5)
    assert(field(factory.create(existing, 11), "left") eq existing)

    println("scala39-immutable-reference-local-pair=ok; identity=true; fresh=true; mutation-isolated=true; seeds=3,11,0,-5")
