package example

object PairFactoryConsumer:
  def main(args: Array[String]): Unit =
    val factory = new PairFactory
    val pair = factory.create(7, 3)
    assert(pair.leftValue() == 7)
    assert(pair.rightValue() == 3)

    val reversed = factory.create(3, 7)
    assert(reversed.leftValue() == 3)
    assert(reversed.rightValue() == 7)

    val leftField = classOf[Pair].getDeclaredField("left")
    val rightField = classOf[Pair].getDeclaredField("right")
    leftField.setAccessible(true)
    rightField.setAccessible(true)
    val left = leftField.get(pair).asInstanceOf[Counter]
    val right = rightField.get(pair).asInstanceOf[Counter]
    assert(left ne right)
    left.set(11)
    assert(pair.leftValue() == 11)
    assert(pair.rightValue() == 3)
    right.set(5)
    assert(pair.leftValue() == 11)
    assert(pair.rightValue() == 5)
