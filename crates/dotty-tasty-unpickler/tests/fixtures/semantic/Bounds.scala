package me.cytrowski.tastyfixtures.semantic

// Type members whose info is a TYPEBOUNDS node: aliases (no upper bound),
// genuine two-sided bounds, and a higher-kinded alias whose type parameter
// variance is written as markers after the bound.
object Bounds {
  trait Low
  trait High extends Low
  class Item

  type Alias = Item
  type Nested = List[Item]
  type Exact >: High <: Low
  trait Bounded[A >: High <: Low]
  type Covariant[+A] = List[A]

  // Wildcards and refinements are where TYPEBOUNDS occurs as a type.
  def upper(x: List[? <: Low]) = x
  def lower(x: List[? >: High]) = x
  def both(x: List[? >: High <: Low]) = x
  def unbounded(x: List[?]) = x
  def aliased(x: { type T = Item }) = x
  def aliasedNested(x: { type T = List[Item] }) = x
  def variant(x: { type F[+A] = List[A] }) = x

  // More variance shapes: an explicit invariant (STABLE), a contravariant
  // parameter, several parameters with mixed variances in order, a variance
  // on the UPPER bound of a two-sided TYPEBOUNDS, and an F-bounded parameter
  // whose bound names the lambda itself.
  trait Ord[T]
  def stable(x: { type F[A] = List[A] }) = x
  def contra(x: { type F[-A] = A => Unit }) = x
  def mixed(x: { type F[+A, B, -C] = (C => A, B) }) = x
  def upperHigher(x: { type F[+A] <: Iterable[A] }) = x
  def fBounded(x: { type F[+A <: Ord[A]] = List[A] }) = x
  def twins(x: { type F[+A] = List[A]; type G[+A] = List[A] }) = x
  // Both bounds are the same lambda instance: the upper one, which carries the
  // marker, is a SHAREDtype link to the lower one, which carries none.
  def sharedLambda(x: { type G[+A] >: List[A] <: List[A] }) = x
}
