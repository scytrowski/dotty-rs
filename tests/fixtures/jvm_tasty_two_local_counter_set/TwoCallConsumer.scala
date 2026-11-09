package client

import example.{Counter, Pair, PairFactory}

object TwoCallConsumer:
  private def field(pair: Pair, name: String): Counter =
    val field = classOf[Pair].getDeclaredField(name)
    field.setAccessible(true)
    field.get(pair).asInstanceOf[Counter]

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    val first = factory.create(7, 3)
    assert(first.leftValue() == 3 && first.rightValue() == 7)
    val firstLeft = field(first, "left")
    val firstRight = field(first, "right")
    assert(firstLeft ne firstRight)

    val second = factory.create(-9, 42)
    assert(second.leftValue() == 42 && second.rightValue() == -9)
    val secondLeft = field(second, "left")
    val secondRight = field(second, "right")
    assert(secondLeft ne secondRight)

    val equal = factory.create(7, 7)
    val equalLeft = field(equal, "left")
    val equalRight = field(equal, "right")
    assert(equalLeft ne equalRight)
    assert(equalLeft.ne(firstLeft) && equalLeft.ne(firstRight))

    firstLeft.set(19)
    assert(first.leftValue() == 19 && first.rightValue() == 7)
    secondRight.set(-5)
    assert(second.leftValue() == 42 && second.rightValue() == -5)
    val fresh = factory.create(7, 3)
    assert(fresh.leftValue() == 3 && fresh.rightValue() == 7)
    assert(first.ne(fresh))
    assert(field(fresh, "left") ne firstLeft)
    assert(field(fresh, "right") ne firstRight)
    println("scala39-immutable-reference-local-pair; two-counter-set-consumer passed")
