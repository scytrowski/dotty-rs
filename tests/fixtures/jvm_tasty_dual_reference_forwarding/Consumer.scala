package example

object PairFactoryConsumer:
  def main(args: Array[String]): Unit =
    val factory = new PairFactory
    val first = new Counter(7)
    val second = new Counter(3)
    val pair = factory.wrap(first, second)
    assert(pair.leftValue() == 7)
    assert(pair.rightValue() == 3)
    val leftField = classOf[Pair].getDeclaredField("left")
    val rightField = classOf[Pair].getDeclaredField("right")
    leftField.setAccessible(true)
    rightField.setAccessible(true)
    assert(leftField.get(pair) eq first)
    assert(rightField.get(pair) eq second)
    first.set(11)
    assert(pair.leftValue() == 11)
    assert(pair.rightValue() == 3)
    second.set(5)
    assert(pair.leftValue() == 11)
    assert(pair.rightValue() == 5)

    val reversed = factory.wrap(second, first)
    assert(reversed.leftValue() == 5)
    assert(reversed.rightValue() == 11)
    assert(leftField.get(reversed) eq second)
    assert(rightField.get(reversed) eq first)

    val alias = factory.wrap(first, first)
    assert(leftField.get(alias) eq rightField.get(alias))
    first.set(19)
    assert(alias.leftValue() == 19)
    assert(alias.rightValue() == 19)
