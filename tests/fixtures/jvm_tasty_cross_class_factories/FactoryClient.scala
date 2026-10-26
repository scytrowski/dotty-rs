package client

import example.CounterFactory

object FactoryClient:
  def main(args: Array[String]): Unit =
    val counter = new CounterFactory().create(7)
    counter.set(42)
    assert(counter.get() == 42)
