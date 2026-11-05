package client

import example.{Counter, Pair, PairFactory}

object Consumer:
  private def field(pair: Pair, name: String): Counter =
    val field = classOf[Pair].getDeclaredField(name)
    field.setAccessible(true)
    field.get(pair).asInstanceOf[Counter]

  def main(args: Array[String]): Unit =
    val factory = new PairFactory()
    val pair = factory.create(7, 3)
    val left = field(pair, "left")
    val right = field(pair, "right")
    assert(pair.leftValue() == 7)
    assert(pair.rightValue() == 3)
    assert(left ne right)
    left.set(41)
    assert(pair.leftValue() == 41 && pair.rightValue() == 3)
    right.set(-8)
    assert(pair.leftValue() == 41 && pair.rightValue() == -8)

    val repeated = factory.create(7, 3)
    val repeatedLeft = field(repeated, "left")
    val repeatedRight = field(repeated, "right")
    assert(repeatedLeft.ne(left) && repeatedRight.ne(right))
    assert(repeatedLeft ne repeatedRight)

    val aliased = new Pair(left, left)
    assert(field(aliased, "left") eq field(aliased, "right"))
