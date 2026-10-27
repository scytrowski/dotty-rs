package client

object CounterOwnerClient:
  def main(args: Array[String]): Unit =
    val counter = new example.Counter(7)
    val owner = new example.CounterOwner(counter)
    assert(owner.read() == 7)
    counter.set(42)
    assert(owner.read() == 42)
    counter.set(-9)
    assert(owner.read() == -9)
    val independent = new example.Counter(3)
    val independentOwner = new example.CounterOwner(independent)
    counter.set(10)
    assert(independentOwner.read() == 3)
