package consumer

object Consumer:
  def run(): Int =
    val original = new example.Counter(start = 7)
    val owner = new example.CounterOwner(counter = original)
    if owner.read() != 7 then throw new AssertionError("initial alias value")
    val inner = owner.getClass.getDeclaredField("inner")
    inner.setAccessible(true)
    if inner.get(owner) ne original then throw new AssertionError("owner replaced injected reference")
    original.set(value = 42)
    if owner.read() != 42 then throw new AssertionError("owner did not retain reference")

    val another = new example.Counter(start = 3)
    val secondOwner = new example.CounterOwner(counter = another)
    if secondOwner.read() != 3 then throw new AssertionError("instances not independent")
    if inner.get(secondOwner) ne another then throw new AssertionError("second owner replaced reference")

    original.set(value = -9)
    if owner.read() != -9 || secondOwner.read() != 3 then
      throw new AssertionError("incorrect object aliasing")
    owner.read()
