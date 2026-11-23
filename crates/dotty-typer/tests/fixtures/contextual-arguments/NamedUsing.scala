package namedctx
trait Tag
class Client:
  def pick(using n: Int, flag: Boolean, text: Tag): Int = n
  def pos(i: Int, b: Boolean, t: Tag): Int = pick(using i, b, t)
  def prefix(using b: Boolean, t: Tag): Int = pick(using 7)
  def ordered(i: Int, b: Boolean, t: Tag): Int = pick(using n = i, flag = b, text = t)
  def reverse(i: Int, b: Boolean, t: Tag): Int = pick(using text = t, flag = b, n = i)
  def legacy(implicit n: Int, flag: Boolean): Int = n
  def tickBool(): Boolean = true
  def tickInt(): Int = 7
  def effects(): Int = legacy(using flag = tickBool(), n = tickInt())
  def hole(using i: Int, t: Tag): Int = pick(using flag = true)
  def mixed(t: Tag)(using b: Boolean): Int = pick(using 7, text = t)
