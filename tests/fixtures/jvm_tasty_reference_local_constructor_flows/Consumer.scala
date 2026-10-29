package example

object FactoryConsumer:
  def main(args: Array[String]): Unit =
    val factory = new CounterOwnerFactory
    val positive = factory.create(7)
    val negative = factory.create(-9)
    val third = factory.create(3)
    assert(positive.read() == 7)
    assert(negative.read() == -9)
    assert(third.read() == 3)

    val firstCounter = factory.makeCounter(7)
    val secondCounter = factory.makeCounter(7)
    assert(firstCounter ne secondCounter)
    firstCounter.set(41)
    assert(firstCounter.get() == 41)
    assert(secondCounter.get() == 7)
    println("scala39-reference-local-constructor-flow=ok")
