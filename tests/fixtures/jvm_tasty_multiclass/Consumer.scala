package consumer

object Consumer:
  def main(args: Array[String]): Unit =
    val counter = new example.Counter(start = 7)
    val gauge = new example.Gauge(start = 2)
    if counter.get() != 7 || gauge.get() != 2 then
      throw new AssertionError("initial state")
    counter.set(value = 13)
    gauge.set(value = 29)
    if counter.get() != 13 || gauge.get() != 29 then
      throw new AssertionError("independent state")
    val secondCounter = new example.Counter(start = 5)
    val secondGauge = new example.Gauge(start = 3)
    if secondCounter.get() != 5 || secondGauge.get() != 3 then
      throw new AssertionError("independent instances")
    println(counter.get() * 100 + gauge.get())
