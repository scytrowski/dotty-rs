package example

object DualReferenceConsumer:
  def main(args: Array[String]): Unit =
    val first = new Counter(7)
    val second = new Counter(3)
    val pair = new Pair(first, second)

    assert(pair.leftValue() == 7)
    assert(pair.rightValue() == 3)
    first.set(70)
    assert(pair.leftValue() == 70)
    assert(pair.rightValue() == 3)
    second.set(30)
    assert(pair.leftValue() == 70)
    assert(pair.rightValue() == 30)

    val swapped = new Pair(second, first)
    assert(swapped.leftValue() == 30)
    assert(swapped.rightValue() == 70)

    val aliased = new Pair(first, first)
    first.set(700)
    assert(aliased.leftValue() == 700)
    assert(aliased.rightValue() == 700)
    println("scala39-dual-reference-constructor=ok")
