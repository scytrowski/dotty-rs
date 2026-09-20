package me.cytrowski.tastyfixtures.semantic

// Type lambdas as they occur inside types (refinement infos), where TASTy
// writes TYPELAMBDAtype and PARAMtype nodes.
object Binders {
  trait Top
  trait Ord[T]
  class Box[F[_]]

  def id(x: { type F = [A] =>> A }) = x
  def upper(x: { type F = [A <: Top] =>> A }) = x
  def outer(x: { type F = [A] =>> [B] =>> A }) = x
  def inner(x: { type F = [A] =>> [B] =>> B }) = x
  def twins(x: { type F = [A] =>> A; type G = [A] =>> A }) = x
  def fBound(x: { type F = [A <: Ord[A]] =>> A }) = x
  def two(x: { type F = [A, B] =>> (A, B) }) = x
  def applied(x: Box[[A] =>> A]) = x
}
