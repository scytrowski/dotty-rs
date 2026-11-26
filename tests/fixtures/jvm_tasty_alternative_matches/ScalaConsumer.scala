object ScalaConsumer {
 def main(args: Array[String]): Unit = {
  val v = new example.AlternativeMatches()
  assert(v.nested(2) == 10)
  assert(v.guarded(0) == 30)
  assert(v.observed() == 12)
  assert(v.longReturn(1, Long.MinValue, Long.MaxValue) == Long.MinValue)
  assert(v.longValue(2, Long.MinValue, Long.MaxValue) == Long.MaxValue)
  println("alternative match oracle ok")
 }
}
