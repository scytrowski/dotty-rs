{
  class Derived derives Base, pkg.Mixin uses cap initially, outer.cap:
    self: Parent =>
      val value = cap
}
