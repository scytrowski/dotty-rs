package namedctx
class Errors:
  def unknown(c: Client)(using i: Int, b: Boolean, t: Tag): Int = c.pick(using bogus = b)
  def duplicate(c: Client)(using i: Int, b: Boolean, t: Tag): Int = c.pick(using n = i, n = i, flag = b)
  def missing(c: Client, t: Tag): Int = c.pick(using flag = true, text = t)
  def ambiguous(c: Client)(using i: Int, j: Int, t: Tag): Int = c.pick(using flag = true)
  def misplaced(c: Client, t: Tag): Int = c.pick(using text = t, true, n = 7)
