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
    assert(pair.leftValue() == 3 && pair.rightValue() == 7)
    assert(left ne right)
    val positional = factory.create(3, 7)
    assert(positional.leftValue() == 7 && positional.rightValue() == 3)
    val equalSeeds = factory.create(7, 7)
    assert(equalSeeds.leftValue() == 7 && equalSeeds.rightValue() == 7)
    assert(field(equalSeeds, "left") ne field(equalSeeds, "right"))
    left.set(41)
    assert(pair.leftValue() == 41 && pair.rightValue() == 7)
    right.set(-8)
    assert(pair.leftValue() == 41 && pair.rightValue() == -8)
    val repeated = factory.create(7, 3)
    assert(field(repeated, "left").ne(left))
    assert(field(repeated, "right").ne(right))
    println("scala39-immutable-reference-local-pair reversed positional arguments")
