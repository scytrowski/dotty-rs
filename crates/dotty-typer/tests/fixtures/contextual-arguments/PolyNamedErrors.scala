package inference
class Errors:
  def unknown(c: Client, b: Box[Int], f: Fmt[Int]): Any = c.choose(using bogus = b, fmt = f)
  def duplicate(c: Client, b: Box[Int]): Any = c.choose(using value = b, value = b)
  def missing(c: Client, b: Box[Int]): Box[Int] = c.choose(using value = b)
  def ambiguous(c: Client, b: Box[Int])(using l: Fmt[Int], r: Fmt[Int]): Box[Int] = c.choose(using value = b)
