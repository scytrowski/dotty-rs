package omitted
class Invalid:
  def absent(c: Client): Any = c.read(using flag = true)
  def noSecond(c: Client)(using b: Box[Int]): Any = c.read(using flag = true)
  def mismatch(c: Client)(using b: Box[Int], f: Fmt[Boolean]): Any = c.read(using flag = true)
  def ambSame(c: Client)(using a: Box[Int], b: Box[Int]): Any = c.one(using flag = true)
  def ambDiff(c: Client)(using a: Box[Int], b: Box[Boolean]): Any = c.one(using flag = true)
  def lateFilter(c: Client)(using a: Box[Int], b: Box[Boolean], f: Fmt[Int]): Any = c.read(using flag = true)
  def boundBad(c: Client)(using i: Box[Int]): Any = c.bounded(using flag = true)
