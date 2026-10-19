package example

class Decisions:
  def unit: Unit = ()
  def enabled: Boolean = true
  def disabled: Boolean = false
  def choose(flag: Boolean, a: Int, b: Int): Int =
    if flag then a else b
  def nested(a: Boolean, b: Boolean): Int =
    if a then
      if b then 1 else 2
    else 3
  def stored(flag: Boolean, value: Int): Int =
    val saved: Int = value
    if flag then saved else 0
