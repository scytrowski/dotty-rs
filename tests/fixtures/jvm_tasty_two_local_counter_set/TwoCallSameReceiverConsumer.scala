package client

import example.{Counter, Pair, PairFactory}

object TwoCallSameReceiverConsumer:
  private def field(pair: Pair, name: String): Counter =
    val field = classOf[Pair].getDeclaredField(name)
    field.setAccessible(true)
    field.get(pair).asInstanceOf[Counter]

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    val first = factory.create(7, 3)
    assert(first.leftValue() == 7 && first.rightValue() == 3)
    val firstLeft = field(first, "left")
    val firstRight = field(first, "right")
    assert(firstLeft ne firstRight)

    val second = factory.create(-9, 42)
    assert(second.leftValue() == -9 && second.rightValue() == 42)
    val secondLeft = field(second, "left")
    val secondRight = field(second, "right")
    assert(secondLeft ne secondRight)

    firstLeft.set(19)
    assert(first.leftValue() == 19 && first.rightValue() == 3)
    assert(second.leftValue() == -9 && second.rightValue() == 42)
    val fresh = factory.create(7, 3)
    assert(fresh.leftValue() == 7 && fresh.rightValue() == 3)
    assert(field(fresh, "left") ne firstLeft)
    println("scala39-immutable-reference-local-pair; same-receiver two-call consumer passed")
