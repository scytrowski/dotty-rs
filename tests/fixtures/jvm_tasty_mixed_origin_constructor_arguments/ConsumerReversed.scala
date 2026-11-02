object ConsumerReversed:
  def main(args: Array[String]): Unit =
    val factory = new example.PairFactory()
    val existing = new example.Counter(7)
    val pair = factory.createAndWrap(existing, 3)
    val repeated = factory.createAndWrap(existing, -9)
    assert(!(pair eq repeated), "each call must return a fresh Pair")
    val left = field(pair, "left")
    val right = field(pair, "right")
    assert(right eq existing, "the second argument must retain identity")
    assert(!(left eq existing), "the nested allocation must be fresh")
    assert(!(left eq field(repeated, "left")), "each nested allocation must be fresh")
    expect(pair.leftValue(), 3)
    expect(pair.rightValue(), 7)
    expect(repeated.leftValue(), -9)
    expect(repeated.rightValue(), 7)
    left.set(30)
    expect(pair.leftValue(), 30)
    expect(pair.rightValue(), 7)
    expect(repeated.leftValue(), -9)
    right.set(70)
    expect(pair.rightValue(), 70)
    expect(repeated.rightValue(), 70)
    println("formal-identity=true; fresh-counter=true; isolated-mutation=true; argument-order=new,existing")

  private def field(pair: example.Pair, name: String): example.Counter =
    val field = classOf[example.Pair].getDeclaredField(name)
    field.setAccessible(true)
    field.get(pair).asInstanceOf[example.Counter]

  private def expect(actual: Int, expected: Int): Unit =
    assert(actual == expected, s"expected $expected, got $actual")
