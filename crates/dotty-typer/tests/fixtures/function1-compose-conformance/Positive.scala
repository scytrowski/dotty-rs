trait ComposeFunction[-T1, +R]:
  def apply(v1: T1): R

  def inner[A](g: A => T1, x: A): Unit =
    val result: T1 = g(x)
    ()

  def outer[A](g: A => T1, x: A): R =
    apply(g(x))

  def compose[A](g: A => T1): A => R =
    x => apply(g(x))

object ComposeControls:
  val f: ComposeFunction[Int, String] = new ComposeFunction[Int, String]:
    def apply(v1: Int): String = v1.toString

  val g: Long => Int = _.toInt
  val inner: Unit = f.inner(g, 1L)
  val outer: String = f.outer(g, 1L)
  val full: Long => String = f.compose(g)
