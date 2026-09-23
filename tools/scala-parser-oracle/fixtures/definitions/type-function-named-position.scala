{
  val f: (x: A) => B = value
  def use(f: (x: A, y: B) => C): D = body
  class Box(callback: (x: A) => B)
  given Ordering[(x: A) => B] = ordering
  f
}
