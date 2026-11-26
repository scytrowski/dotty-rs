object ScalaConsumer {
 def main(args: Array[String]): Unit = {
  val p = new example.FloatingValues()
  assert(java.lang.Float.floatToRawIntBits(p.fnegativeZero()) == 0x80000000)
  assert(java.lang.Double.doubleToRawLongBits(p.dnegativeZero()) == 0x8000000000000000L)
  assert(p.matchedF(1, true, 1.5f, 3.5f) == 1.5f)
  assert(p.matchedD(1, false, 1.5, 3.5) == 3.5)
  assert(p.deadD(true, 1.5, 3.5) == 1.5)
  println("floating value oracle ok")
 }
}
