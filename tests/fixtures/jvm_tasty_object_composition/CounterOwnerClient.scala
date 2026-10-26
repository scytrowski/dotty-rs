package client

object CounterOwnerClient:
  def main(args: Array[String]): Unit =
    val first = new example.CounterOwner(7)
    val second = new example.CounterOwner(42)
    assert(first.read() == 7)
    assert(second.read() == 42)
