package client

import example.{Counter, Pair, PairFactory}

object AliasConsumer:
  private def field(pair: Pair, name: String): Counter =
    val field = classOf[Pair].getDeclaredField(name)
    field.setAccessible(true)
    field.get(pair).asInstanceOf[Counter]

  private def check(pair: Pair, left: Int, right: Int): (Counter, Counter) =
    assert(pair.leftValue() == left && pair.rightValue() == right)
    val leftCounter = field(pair, "left")
    val rightCounter = field(pair, "right")
    assert(leftCounter ne rightCounter)
    (leftCounter, rightCounter)

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    val first = factory.create(7, 3)
    val (firstLeft, firstRight) = check(first, 3, 7)
    assert(firstLeft.get() == 3 && firstRight.get() == 7)

    val second = factory.create(-9, 42)
    val (secondLeft, secondRight) = check(second, 42, -9)
    assert(secondLeft ne firstLeft)
    assert(secondRight ne firstRight)

    firstLeft.set(19)
    assert(first.leftValue() == 19 && first.rightValue() == 7)
    assert(second.leftValue() == 42 && second.rightValue() == -9)

    secondRight.set(-5)
    assert(second.leftValue() == 42 && second.rightValue() == -5)
    assert(first.leftValue() == 19 && first.rightValue() == 7)

    val fresh = factory.create(7, 3)
    val (freshLeft, freshRight) = check(fresh, 3, 7)
    assert((fresh ne first) && (fresh ne second))
    assert((freshLeft ne firstLeft) && (freshLeft ne secondLeft))
    assert((freshRight ne firstRight) && (freshRight ne secondRight))

    val equalSeeds = factory.create(11, 11)
    val (equalLeft, equalRight) = check(equalSeeds, 11, 11)
    assert(equalLeft ne equalRight)
    equalLeft.set(12)
    assert(equalSeeds.leftValue() == 12 && equalSeeds.rightValue() == 11)
    println("scala39-immutable-reference-local-pair; rust-alias-two-call-consumer passed")
