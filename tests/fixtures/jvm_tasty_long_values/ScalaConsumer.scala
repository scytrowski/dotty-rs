import example.LongValues
object ScalaConsumer {
  def main(args: Array[String]): Unit = {
    val value = new LongValues()
    assert(value.literal() == 9000000000L)
    assert(value.minimum() == Long.MinValue)
    assert(value.maximum() == Long.MaxValue)
    assert(value.direct(Long.MaxValue) == Long.MaxValue)
    assert(value.mixed(7, Long.MinValue, true, Long.MaxValue) == Long.MinValue)
    assert(value.saved(Long.MinValue) == Long.MinValue)
    assert(value.rotate(2, Long.MinValue, Long.MaxValue) == Long.MaxValue)
    assert(value.choose(0, Long.MinValue, Long.MaxValue) == Long.MinValue)
    assert(value.returned(0, Long.MinValue, Long.MaxValue) == Long.MinValue)
    assert(value.once(1L, 9000000000L) == 9000000000L)
    assert(value.observed() == 12)
    println("long value oracle ok")
  }
}
