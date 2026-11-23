package namedctx
class Errors:
  def unknown(c: Client)(using i: Int, b: Boolean, t: Tag): Int = c.pick(using bogus = b)
  def duplicate(c: Client)(using i: Int, b: Boolean, t: Tag): Int = c.pick(using n = i, n = i, flag = b)
