package example
class BooleanCalls {
  def positive(x: Int): Boolean = x > 0
  def identity(b: Boolean): Boolean = b
  def both(a: Boolean, b: Boolean): Boolean = a & b
  def ready(): Boolean = true
  def direct(x: Int): Boolean = positive(x)
  def explicit(x: Int): Boolean = this.positive(x)
  def noarg(): Boolean = ready()
  def nested(x: Int): Boolean = identity(positive(x))
  def mixed(x: Int, b: Boolean): Boolean = b && positive(x)
  def mixedCall(x: Int, b: Boolean): Boolean = mixed(x, b)
  def condition(x: Int): Int = if positive(x) then 1 else 2
  def not(x: Int): Boolean = !positive(x)
  def local(x: Int): Boolean = { val b = positive(x); b }
  def selector(x: Int): Int = positive(x) match { case true => 1; case false => 2 }
  def guard(x: Int): Int = x match { case n if positive(n) => 1; case _ => 2 }
  def loop(x: Int): Int = { var n: Int = x; while positive(n) do n = n - 1; n }
  def consume(b: Boolean): Int = if b then 1 else 2
  def intCall(x: Int): Int = consume(positive(x))
  def dangerous(x: Int): Boolean = 10 / x > 0
  def eager(a: Boolean, x: Int): Boolean = a & dangerous(x)
  def shortAnd(a: Boolean, x: Int): Boolean = a && dangerous(x)
  def shortOr(a: Boolean, x: Int): Boolean = a || dangerous(x)
  def discard(x: Int): Unit = dangerous(x)
  def returns(x: Int): Boolean = { if positive(x) then return identity(true); false }
  def ordered(a: Boolean, b: Boolean): Int = { var order: Int = 0; both({ order = order * 10 + 1; a }, { order = order * 10 + 2; b }); order }
  def conditionalArg(a: Boolean, b: Boolean): Boolean = identity(if a then b else !b)
}
