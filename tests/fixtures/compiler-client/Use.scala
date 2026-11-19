package demo

class Use:
  def fromA(a: A) = a.value()
  def fromB(b: B) = b.value()
  def diff(left: Int, right: Int): Int = { val delta = left - right; delta }
