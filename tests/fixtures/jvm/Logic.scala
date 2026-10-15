class Logic {
  def both(a: Boolean, b: Boolean): Boolean = a && b
  def either(a: Boolean, b: Boolean): Boolean = a || b
  def negate(x: Boolean): Boolean = !x
  def exclusive(a: Boolean, b: Boolean): Boolean = a ^ b
  def equal(a: Boolean, b: Boolean): Boolean = a == b
  def notEqual(a: Boolean, b: Boolean): Boolean = a != b

  def skipRight(): Boolean = false && (1 / 0 > 0)
  def skipOrRight(): Boolean = true || (1 / 0 > 0)
  def runOrRight(): Boolean = false || (1 / 0 > 0)
  def runAndRight(): Boolean = true && (1 / 0 > 0)
  def eagerAndRight(): Boolean = false & (1 / 0 > 0)
  def eagerOrRight(): Boolean = true | (1 / 0 > 0)
  def eagerXorRight(): Boolean = true ^ (1 / 0 > 0)

  def countRight(flag: Boolean): Int = {
    var hits = 0
    val result = flag && {
      hits = hits + 1
      true
    }
    hits
  }

  def countStrictAnd(a: Boolean, b: Boolean): Int = {
    var hits = 0
    val result = ({ hits = hits + 1; a } & { hits = hits + 1; b })
    hits
  }

  def countStrictOr(a: Boolean, b: Boolean): Int = {
    var hits = 0
    val result = ({ hits = hits + 1; a } | { hits = hits + 1; b })
    hits
  }

  def countStrictXor(a: Boolean, b: Boolean): Int = {
    var hits = 0
    val result = ({ hits = hits + 1; a } ^ { hits = hits + 1; b })
    hits
  }
}
