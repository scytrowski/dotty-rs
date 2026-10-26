package consumer

object Consumer:
  def run(): Int =
    val a = new example.CounterOwner(seed = 7)
    val b = new example.CounterOwner(seed = 42)
    if a.read() != 7 || b.read() != 42 then
      throw new AssertionError("bad owned field initialization")
    a.read() * 100 + b.read()
