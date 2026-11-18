package clients
import defaults.Reader

class AmbiguousClient:
  def ambiguous(reader: Reader)(using a: Int, b: Int): Int = reader.offset(7)
