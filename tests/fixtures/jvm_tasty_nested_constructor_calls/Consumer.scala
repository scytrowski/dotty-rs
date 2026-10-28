package client

object Consumer:
  def main(args: Array[String]): Unit =
    val factory = new example.CounterOwnerFactory
    assert(factory.create(7).read() == 7)
    assert(factory.create(3).read() == 3)
    println("scala39-nested-constructor-oracle=ok")
