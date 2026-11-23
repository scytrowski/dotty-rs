package inference
class Errors:
  def unknown(c: Client, b: Box[Int], f: Fmt[Int]): Any = c.choose(using bogus = b, fmt = f)
  def duplicate(c: Client, b: Box[Int]): Any = c.choose(using value = b, value = b)
