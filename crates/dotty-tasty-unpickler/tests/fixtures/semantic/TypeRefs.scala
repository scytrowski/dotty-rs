package me.cytrowski.tastyfixtures.semantic

// Everything a reference must resolve by definition address lives in one unit:
// `Left` and `Right` each have a member class `Inner`, so a reference to
// `Inner` that was resolved by name could pick the wrong one.
object Distinct {
  class Left {
    class Inner
    def make: Inner = new Inner
  }

  class Right {
    class Inner
    def make: Inner = new Inner
  }

  def cross(x: Left#Inner, y: Right#Inner): Int = 0
  def same(x: Left#Inner, y: Left#Inner): Int = 0

  class Box[T](val value: T) {
    def get: T = value
  }

  val one: Int = 1
  def use: one.type = one

  def local(): Int = {
    class Hidden
    val h: Hidden = new Hidden
    0
  }
}
