object Consumer:
  def main(args: Array[String]): Unit =
    val factory = new example.PairFactory()
    val existing = new example.Counter(7)
    val normal = factory.wrapAndCreate(existing, 3)
    val negative = factory.wrapAndCreate(existing, -9)
    assert(!(normal eq negative), "each call must return a fresh Pair")
    val normalLeft = field(normal, "left")
    val normalRight = field(normal, "right")
    val negativeRight = field(negative, "right")
    assert(normalLeft eq existing, "the first argument must retain identity")
    assert(!(normalRight eq existing), "the nested allocation must be fresh")
    assert(!(normalRight eq negativeRight), "each nested allocation must be fresh")
    expect(normal.leftValue(), 7)
    expect(normal.rightValue(), 3)
    expect(negative.leftValue(), 7)
    expect(negative.rightValue(), -9)
    normalLeft.set(70)
    expect(normal.leftValue(), 70)
    expect(negative.leftValue(), 70)
    expect(normal.rightValue(), 3)
    normalRight.set(30)
    expect(normal.rightValue(), 30)
    expect(negative.rightValue(), -9)
    println("formal-identity=true; fresh-counter=true; isolated-mutation=true; argument-order=existing,new")

  private def field(pair: example.Pair, name: String): example.Counter =
    val field = classOf[example.Pair].getDeclaredField(name)
    field.setAccessible(true)
    field.get(pair).asInstanceOf[example.Counter]

  private def expect(actual: Int, expected: Int): Unit =
    assert(actual == expected, s"expected $expected, got $actual")
