package consumer

object Consumer:
  def run(): Int =
    val factory = new example.CounterFactory()
    val first = factory.create(seed = 7)
    val second = factory.create(seed = 2)
    if first.get() != 7 || second.get() != 2 then
      throw new AssertionError("bad constructor state")
    first.set(value = 42)
    if first.get() != 42 || second.get() != 2 then
      throw new AssertionError("instances not independent")
    first.get()

  def main(args: Array[String]): Unit =
    println(run())
