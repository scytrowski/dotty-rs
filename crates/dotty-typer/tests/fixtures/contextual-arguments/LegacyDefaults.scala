package example

class LegacyDefaults:
  def legacy(implicit n: Int = 7): Int = n
  def fallback(): Int = legacy
  def supplied(using chosen: Int): Int = legacy
  def explicit(using a: Int, b: Int): Int = legacy(9)
