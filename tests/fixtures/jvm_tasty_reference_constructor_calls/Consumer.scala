package consumer

import example.Counter
import example.CounterOwnerFactory

object CounterOwnerFactoryConsumer:
  def main(args: Array[String]): Unit =
    val factory = new CounterOwnerFactory()
    val first = new Counter(7)
    val firstOwner = factory.wrap(first)
    assert(firstOwner.read() == 7)
    first.set(42)
    assert(firstOwner.read() == 42)

    val second = new Counter(3)
    val secondOwner = factory.wrap(second)
    first.set(-9)
    assert(firstOwner.read() == -9)
    assert(secondOwner.read() == 3)
    println("scala39-reference-constructor-alias=ok")
