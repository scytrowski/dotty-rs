package example
class EarlyReturns {
  private var hits: Int = 0
  def tick(): Int = { hits = hits + 1; hits }
  def observed(): Int = hits
  def direct(x: Int): Int = return x
  def booleanValue(flag: Boolean): Boolean = return flag
  def unit(): Unit = return
  def choose(x: Int): Int = { if x < 0 then return tick(); tick() + 10 }
  def both(x: Int): Int = if x < 0 then return -1 else return x + 1
  def matched(x: Int): Int = x match {
    case n if n < 0 => return n
    case n => return n + 1
  }
  def inLoop(x: Int): Int = {
    var n: Int = x
    while n > 0 do { if n == 3 then return n; n = n - 1 }
    0
  }
  def stop(flag: Boolean): Unit = { if flag then return; hits = hits + 100 }
  def positive(x: Int): Boolean = { if x < 0 then return false; true }
  def once(): Int = { return tick(); tick() }
  def deadIf(flag: Boolean): Int = if flag then { return 7; tick() } else 3
  def deadMatch(x: Int): Int = x match { case 0 => { return 7; tick() }; case _ => 3 }
  def deadBoth(flag: Boolean): Int = if flag then { return 1; tick() } else { return 2; tick() }
}
