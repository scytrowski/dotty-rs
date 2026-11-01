package example

object RustArtifactConsumer:
  def main(args: Array[String]): Unit =
    val factory = new PairFactory
    val first = factory.create(7, 3)
    val reversed = factory.create(3, 7)
    val signed = factory.create(-9, 42)
    assert(first.leftValue() == 7 && first.rightValue() == 3)
    assert(reversed.leftValue() == 3 && reversed.rightValue() == 7)
    assert(signed.leftValue() == -9 && signed.rightValue() == 42)

    val leftField = classOf[Pair].getDeclaredField("left")
    val rightField = classOf[Pair].getDeclaredField("right")
    leftField.setAccessible(true)
    rightField.setAccessible(true)
    val firstLeft = leftField.get(first).asInstanceOf[Counter]
    val firstRight = rightField.get(first).asInstanceOf[Counter]
    val reversedLeft = leftField.get(reversed).asInstanceOf[Counter]
    val reversedRight = rightField.get(reversed).asInstanceOf[Counter]
    val signedLeft = leftField.get(signed).asInstanceOf[Counter]
    val signedRight = rightField.get(signed).asInstanceOf[Counter]
    assert(Array(firstLeft, firstRight, reversedLeft, reversedRight, signedLeft, signedRight).distinct.length == 6)
    firstLeft.set(100)
    assert(first.leftValue() == 100 && first.rightValue() == 3)
    assert(reversed.leftValue() == 3 && signed.leftValue() == -9)
    firstRight.set(-5)
    assert(first.leftValue() == 100 && first.rightValue() == -5)
    reversedLeft.set(-100)
    assert(first.leftValue() == 100 && first.rightValue() == -5)
    assert(reversed.leftValue() == -100 && reversed.rightValue() == 7)
    assert(signed.leftValue() == -9 && signed.rightValue() == 42)

    val direct = new Counter(11)
    direct.set(12)
    assert(direct.get() == 12)
    val directPair = new Pair(new Counter(5), new Counter(6))
    assert(directPair.leftValue() == 5 && directPair.rightValue() == 6)
    println("rust-artifact-scala-consumer=7/3,3/7,-9/42; independent=true")
