package namedctx
trait Tag
class Client:
  def pick(using n: Int, flag: Boolean, text: Tag): Int = n
  def pos(i: Int, b: Boolean, t: Tag): Int = pick(using i, b, t)
  def prefix(using b: Boolean, t: Tag): Int = pick(using 7)
  def ordered(i: Int, b: Boolean, t: Tag): Int = pick(using n = i, flag = b, text = t)
