package inference
class Errors:
  def unknown(c: Client, b: Box[Int], f: Fmt[Int]): Any = c.choose(using bogus = b, fmt = f)
  def duplicate(c: Client, b: Box[Int]): Any = c.choose(using value = b, value = b)
  def missing(c: Client, b: Box[Int]): Box[Int] = c.choose(using value = b)
  def ambiguous(c: Client, b: Box[Int])(using l: Fmt[Int], r: Fmt[Int]): Box[Int] = c.choose(using value = b)
  def clash(c: Client, b: Box[Int], f: Fmt[Boolean]): Any = c.choose(using value = b, fmt = f)
  def badBound(c: Client, b: Box[Int]): Any = c.bounded(using value = b)
