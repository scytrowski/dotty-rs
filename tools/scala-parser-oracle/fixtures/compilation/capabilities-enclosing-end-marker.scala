object Capabilities:
  class Internalize(binder: MethodType)(using Context) extends BiTypeMap:
    thisMap =>

    val sym = ctx.owner
    val paramSyms = atPhase(ctx.phase.prev):
      sym.paramSymss.head
    val resultToAny = EqHashMap[ResultCap, LocalCap]()
    val anyToResult = EqHashMap[LocalCap, ResultCap]()

    override def apply(t: Type) =
      if variance < 0 then t
      else t match
        case t: ParamRef =>
          if t.binder == this.binder then paramSyms(t.paramNum).termRef else t
        case _ => mapOver(t)

    override def mapCapability(c: Capability, deep: Boolean): Capability = c match
      case r: ResultCap if r.binder == this.binder =>
        resultToAny.get(r) match
          case Some(f) => f
          case None =>
            val f = LocalCap(Origin.LocalInstance(binder.resType))
            resultToAny(r) = f
            anyToResult(f) = r
            f
      case _ => super.mapCapability(c, deep)

    class Inverse extends BiTypeMap:
      def apply(t: Type): Type =
        if variance < 0 then t
        else t match
          case t: TermRef if paramSyms.contains(t) =>
            binder.paramRefs(paramSyms.indexOf(t.symbol))
          case _ => mapOver(t)

      override def mapCapability(c: Capability, deep: Boolean) = c match
        case f: LocalCap if f.owner == sym =>
          anyToResult.get(f) match
            case Some(r) => r
            case None =>
              val r = ResultCap(binder)
              resultToAny(r) = f
              anyToResult(f) = r
              r
        case _ => super.mapCapability(c, deep)

      def inverse = thisMap
      override def toString = thisMap.toString + ".inverse"
    end Inverse

    override def toString = "InternalizeClosureResult"
    def inverse = Inverse()
  end Internalize

  def resultToAny(tp: Type, mt: MethodicType, sym: Symbol, fail: Message => Unit)(using Context): Type =
    ToResult(tp, mt, sym, fail)(tp)
end Capabilities
