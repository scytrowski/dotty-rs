package me.cytrowski.tastyfixtures.semantic

class Access {
  private[semantic] def inPackage: Int = 1
  protected[semantic] def protectedInPackage: Int = 2
  private[Access] def inClass: Int = 3
  private[this] def objectPrivate: Int = 4
  private def plain: Int = 5
  protected def guarded: Int = 6
}

class Outer {
  class Inner {
    private[Outer] def withinOuter: Int = 1
  }
}
