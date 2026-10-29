package lexicalclosure

object Positive:
  val identity: Int => Int = x => x

  def parameter(delta: Int): Int => Int = x => x + delta + delta

  def local: Int => Int =
    val delta: Int = 2
    (x: Int) => x + delta

  def shadow(delta: Int): Int => Int = (delta: Int) => delta

  class C:
    val base: Int = 40
    def explicitThis: Int => C = _ => this
    def explicitMember: Int => Int = x => x + this.base
    def implicitMember: Int => Int = x => x + base
