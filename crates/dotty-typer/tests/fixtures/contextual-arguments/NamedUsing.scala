package namedctx
trait Family { type Out }
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
  def fallback(using n: Int = 3, flag: Boolean): Int = n
  def useDefault(): Int = fallback(using flag = true)
  def preferGiven(using i: Int): Int = fallback(using flag = true)
  def overrideDefault(using i: Int): Int = fallback(using flag = true, n = i)
  def read(using f: Family, v: f.Out): f.Out = v
  def dep(using p: Family, w: p.Out): p.Out = read(using v = w, f = p)
  def omitFamily(using p: Family, w: p.Out): p.Out = read(using v = w)
