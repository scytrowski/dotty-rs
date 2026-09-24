opaque type UserId = Long
opaque type Id[A] = A
opaque type Upper <: Any = Impl
opaque type Both >: Lower <: Upper = Impl
