package consumer

object ScalaConsumer:
  def run(): String =
    val factory = new example.CounterOwnerFactory
    val positive = factory.create(7)
    val negative = factory.create(-9)
    val repeated = factory.create(3)
    assert(positive.read() == 7)
    assert(negative.read() == -9)
    assert(repeated.read() == 3)
    assert(!(positive eq negative))
    assert(!(positive eq repeated))
    assert(!(negative eq repeated))
    s"${positive.read()},${negative.read()},${repeated.read()}"
