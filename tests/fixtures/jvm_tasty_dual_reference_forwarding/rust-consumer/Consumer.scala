package example

object RustArtifactConsumer:
  def main(args: Array[String]): Unit =
    val factory = new PairFactory
    val first = new Counter(7)
    val second = new Counter(3)
    val pair = factory.wrap(first, second)
    assert(pair.leftValue() == 7 && pair.rightValue() == 3)

    val leftField = classOf[Pair].getDeclaredField("left")
    val rightField = classOf[Pair].getDeclaredField("right")
    leftField.setAccessible(true)
    rightField.setAccessible(true)
    assert(leftField.get(pair).asInstanceOf[AnyRef] eq first)
    assert(rightField.get(pair).asInstanceOf[AnyRef] eq second)

    first.set(42)
    assert(pair.leftValue() == 42 && pair.rightValue() == 3)
    second.set(-9)
    assert(pair.leftValue() == 42 && pair.rightValue() == -9)

    val reversed = factory.wrap(second, first)
    assert(reversed.leftValue() == -9 && reversed.rightValue() == 42)
    assert(leftField.get(reversed).asInstanceOf[AnyRef] eq second)
    assert(rightField.get(reversed).asInstanceOf[AnyRef] eq first)

    val aliased = new Counter(11)
    val aliasPair = factory.wrap(aliased, aliased)
    assert(leftField.get(aliasPair).asInstanceOf[AnyRef] eq aliased)
    assert(rightField.get(aliasPair).asInstanceOf[AnyRef] eq aliased)
    aliased.set(12)
    assert(aliasPair.leftValue() == 12 && aliasPair.rightValue() == 12)

    println("rust-dual-reference-forwarding=7/3,42/-9,-9/42; alias=true")
