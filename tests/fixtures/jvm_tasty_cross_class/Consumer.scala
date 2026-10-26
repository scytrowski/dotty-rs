package consumer

object Consumer:
  def run(): Int =
    val counter = new example.Counter(start = 7)
    val reader = new example.Reader()
    val before = reader.read(counter = counter)
    counter.set(value = 13)
    val after = reader.read(counter = counter)

    val independent = new example.Counter(start = -5)
    if reader.read(counter = independent) != -5 then
      throw new AssertionError("independent counter initial state")
    counter.set(value = 99)
    if reader.read(counter = independent) != -5 then
      throw new AssertionError("counter state leaked between instances")

    before * 100 + after

  def main(args: Array[String]): Unit = println(run())
