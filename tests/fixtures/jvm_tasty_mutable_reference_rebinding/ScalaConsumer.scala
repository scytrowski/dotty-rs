package consumer

import example.Counter
import example.Pair
import example.PairFactory

object ScalaConsumer:
  private def counter(pair: Pair, name: String): Counter =
    val field = classOf[Pair].getDeclaredField(name)
    field.setAccessible(true)
    field.get(pair).asInstanceOf[Counter]

  private def check(firstSeed: Int, secondSeed: Int, nextValue: Int): Unit =
    val pair: Pair = new PairFactory().create(firstSeed, secondSeed, nextValue)
    val first = counter(pair, "left")
    val second = counter(pair, "right")
    assert(first ne second)
    assert(first.get() == firstSeed)
    assert(second.get() == nextValue)
    first.set(firstSeed + 101)
    assert(pair.leftValue() == firstSeed + 101)
    assert(pair.rightValue() == nextValue)
    second.set(secondSeed - 103)
    assert(pair.leftValue() == firstSeed + 101)
    assert(pair.rightValue() == secondSeed - 103)

  def main(args: Array[String]): Unit =
    check(7, 3, 42)
    check(-9, 11, 42)
    check(5, 5, -13)
    println("scala39-rust-mutable-reference-rebinding=ok")
