package consumer

import example.*

object FactoryAuditConsumer:
  def main(args: Array[String]): Unit =
    val factory = new CounterOwnerFactory
    val first = factory.create(7)
    assert(first.read() == 7)
    val second = factory.create(-9)
    assert(second.read() == -9)
    val third = factory.create(3)
    assert(third.read() == 3)
    val firstCounter = factory.makeCounter(7)
    val secondCounter = factory.makeCounter(7)
    assert(!(firstCounter eq secondCounter))
    firstCounter.set(41)
    assert(firstCounter.get() == 41)
    assert(secondCounter.get() == 7)
    println("scala39-factory-result-arguments=ok")
