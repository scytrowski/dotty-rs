package client

import example.{Counter, Pair, PairFactory}

object Consumer:
  private def field(pair: Pair, name: String): Counter =
    val field = classOf[Pair].getDeclaredField(name)
    field.setAccessible(true)
    field.get(pair).asInstanceOf[Counter]

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    val first = factory.create(7, 3)
    assert(first.leftValue() == 3 && first.rightValue() == 3)
    val firstLeft = field(first, "left")
    val firstRight = field(first, "right")
    assert(firstLeft ne firstRight)
    assert(firstLeft.get() == 3 && firstRight.get() == 3)
    firstLeft.set(19)
    assert(first.leftValue() == 19 && first.rightValue() == 3)

    val second = factory.create(-9, 42)
    assert(second.leftValue() == 42 && second.rightValue() == 42)
    val secondLeft = field(second, "left")
    val secondRight = field(second, "right")
    assert(secondLeft ne secondRight)
    assert(secondLeft.get() == 42 && secondRight.get() == 42)
    secondRight.set(-5)
    assert(second.leftValue() == 42 && second.rightValue() == -5)
    println("scala39-immutable-reference-local-pair; counter-set-consumer passed")
