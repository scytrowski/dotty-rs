package me.cytrowski.tastyfixtures.semantic

// Method and polymorphic types as they occur inside types (refinement infos),
// where TASTy writes METHODtype, POLYtype and PARAMtype nodes.
object Methodic {
  trait Box { type Out }
  trait Ord[T]
  // A polymorphic refinement method needs a matching member in the parent.
  trait Gen {
    def id[A](x: A): A
    def use[A <: Ord[A]](x: A): A
    def f[A](a: A)(b: A): A
  }

  def plain(x: { def run(x: Int): Boolean }) = x
  def empty(x: { def run(): Boolean }) = x
  def contextual(x: { def run(using x: Int): Boolean }) = x
  def legacy(x: { def run(implicit x: Int): Boolean }) = x
  def two(x: { def run(a: Int, b: Boolean): Long }) = x
  def generic(x: Gen { def id[A](x: A): A }) = x
  def fBound(x: Gen { def use[A <: Ord[A]](x: A): A }) = x
  def dependent(x: { def get(x: Box): x.Out }) = x
  def curried(x: { def get(x: Box)(y: x.Out): x.Out }) = x
  def polyCurried(x: Gen { def f[A](a: A)(b: A): A }) = x
  def shared(x: { def f(a: Int): Int; def g(a: Int): Int }) = x
}
