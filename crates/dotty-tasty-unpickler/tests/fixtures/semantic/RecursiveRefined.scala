package me.cytrowski.tastyfixtures.semantic

// Refinement types: term and type members, and recursive refinements (a
// refinement that refers to `this`, written as RECtype / RECthis).
object RecursiveRefined {
  trait Base {
    type T
    type U
    def run(x: Int): Int
    def me: Base
  }

  trait C {
    type T1
    type T2
  }

  def typeMember(x: Base { type T = Int }) = x
  def upperMember(x: Base { type T <: Any }) = x
  def methodMember(x: Base { def run(x: Int): Int }) = x
  def twoMembers(x: Base { type T = Int; def run(x: Int): Int }) = x
  def recursive(x: C { type T1; type T2 = T1 }) = x
  def selfType(x: Base { def me: this.type }) = x
  // The second refinement's info is the same bounds instance: a SHAREDtype link.
  def sharedBounds(x: Base { type T = Int; type U = Int }) = x
  def dependent(x: Base { type T = Int; type U = T }) = x
}
