{
  class ConstructorFinalModifiers(
      final val immutable: Int,
      final var mutable: Int,
      final protected val finalProtected: Int,
      protected final val protectedFinal: Int,
      abstract val abstractAccessor: Int,
      sealed val sealedAccessor: Int,
      implicit val implicitAccessor: Int,
      lazy val lazyAccessor: Int,
      inline val inlineAccessor: Int,
      transparent val transparentAccessor: Int,
      open val openAccessor: Int,
      infix val infixAccessor: Int
  )
}
