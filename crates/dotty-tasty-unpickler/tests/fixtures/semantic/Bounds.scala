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
}
