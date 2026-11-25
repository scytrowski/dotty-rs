package demo
class Probe {
  def count(): Int = 4
  def seed(): Int = 9
  def id(): Int = 2
  def make(probe: Probe): Counter =
    new Counter(count = probe.count(), seed = probe.seed(), id = probe.id())
  def update(c: Counter, n: Int): Unit = c.count = n
  def read(c: Counter): Int = c.count
}
