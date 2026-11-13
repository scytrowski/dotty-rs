package consumer

import example.Counter
import example.Pair
import example.PairFactory

object ScalaConsumer:
  private def check(
      factory: PairFactory,
      firstSeed: Int,
      secondSeed: Int,
      chooseFirst: Boolean,
      nextValue: Int,
      expectedLeft: Int,
      expectedRight: Int
  ): Unit =
    val pair: Pair = factory.create(firstSeed, secondSeed, chooseFirst, nextValue)
    assert(pair.leftValue() == expectedLeft)
    assert(pair.rightValue() == expectedRight)

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    check(factory, 7, 3, true, 42, 42, 3)
    check(factory, 7, 3, false, 42, 7, 42)
    check(factory, -9, 42, true, 0, 0, 42)
    check(factory, -9, 42, false, 0, -9, 0)
    check(factory, 7, 7, true, 42, 42, 7)
    val first = new Counter(11)
    val second = new Counter(5)
    val directPair: Pair = new Pair(first, second)
    assert(directPair.leftValue() == 11)
    assert(directPair.rightValue() == 5)
    first.set(12)
    second.set(-8)
    assert(directPair.leftValue() == 12)
    assert(directPair.rightValue() == -8)
    println("scala39-rust-conditional-reference-consumer=ok")
