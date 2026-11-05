package client

import example.{Counter, Pair, PairFactory}

object Consumer:
  private def field(pair: Pair, name: String): Counter =
    val field = classOf[Pair].getDeclaredField(name)
    field.setAccessible(true)
    field.get(pair).asInstanceOf[Counter]

  def main(args: Array[String]): Unit =
    val pair = new PairFactory().create(7, 3)
    assert(pair.leftValue() == 7 && pair.rightValue() == 7)
    assert(field(pair, "left") eq field(pair, "right"))
    println("scala39-immutable-reference-local-pair alias repeated first local")
