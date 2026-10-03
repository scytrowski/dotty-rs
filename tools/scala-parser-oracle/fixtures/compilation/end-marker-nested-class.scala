object Capabilities:
  class GlobalCapToLocal(origin: Origin)(using Context) extends BiTypeMap, FollowAliasesMap:
    thisMap =>
    override def apply(t: Type) = t match
      case _ => mapOver(t)
    override def mapCapability(c: Capability, deep: Boolean): Capability = c match
      case GlobalAny => LocalCap(origin)
      case _ => super.mapCapability(c, deep)
    class Inverse extends BiTypeMap, FollowAliasesMap:
      def inverse = thisMap
    lazy val inverse = Inverse()
  end GlobalCapToLocal
end Capabilities
