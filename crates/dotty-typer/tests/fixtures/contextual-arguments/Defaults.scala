package defaults

class Reader:
  def read(using n: Int = 7): Int = n
  def offset(x: Int)(using n: Int = x): Int = n

class Client:
  def forwarded(r: Reader)(using n: Int): Int = r.read
  def fallback(r: Reader): Int = r.read
  def previous(r: Reader, x: Int): Int = r.offset(x)
