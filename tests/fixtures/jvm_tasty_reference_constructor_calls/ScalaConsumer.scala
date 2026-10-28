package consumer

object Consumer:
  def run(): Int =
    val original = new example.Counter(start = 7)
    val factory = new example.CounterOwnerFactory()
    val owner = factory.wrap(counter = original)
    if owner.read() != 7 then throw new AssertionError("initial wrong")
    original.set(value = 42)
    if owner.read() != 42 then throw new AssertionError("reference not preserved")
    val another = new example.Counter(start = 3)
    val anotherOwner = factory.wrap(counter = another)
    if anotherOwner.read() != 3 then throw new AssertionError("other graph wrong")
    original.set(value = -9)
    if owner.read() != -9 || anotherOwner.read() != 3 then
      throw new AssertionError("alias graph wrong")
    owner.read()
