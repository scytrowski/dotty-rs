package consumer

import example.*

object FactoryResultConsumer:
  def run(): String =
    val factory = new CounterOwnerFactory
    val positive: CounterOwner = factory.create(7)
    val negative: CounterOwner = factory.create(-9)
    val repeated: CounterOwner = factory.create(3)
    assert(positive.read() == 7)
    assert(negative.read() == -9)
    assert(repeated.read() == 3)
    val firstCounter: Counter = factory.makeCounter(7)
    val secondCounter: Counter = factory.makeCounter(7)
    assert(!(firstCounter eq secondCounter))
    firstCounter.set(41)
    assert(firstCounter.get() == 41)
    assert(secondCounter.get() == 7)
    s"${positive.read()},${negative.read()},${repeated.read()}"
