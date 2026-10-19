package example

class Flags:
  def enabled: Boolean = true
  def disabled: Boolean = false
  def passthrough(flag: Boolean): Boolean = flag
  def keep(flag: Boolean, value: Int): Int = value
