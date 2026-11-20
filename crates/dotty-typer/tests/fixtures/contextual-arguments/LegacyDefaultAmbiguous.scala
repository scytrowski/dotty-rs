class LegacyDefaultAmbiguous:
  def legacy(implicit n: Int = 7): Int = n
  def ambiguous(using a: Int, b: Int): Int = legacy
