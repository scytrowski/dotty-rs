object EndMarkerNestedLocalMatch {
  def occursAtToplevel(param: TypeParamRef, inst: Type)(using Context): Boolean =
    def occurs(tp: Type)(using Context): Boolean = tp match
      case tp: AndOrType => occurs(tp.tp1) || occurs(tp.tp2)
      case tp: TypeParamRef => (tp eq param) || entry(tp).match
        case NoType => false
        case TypeBounds(lo, hi) => (lo eq hi) && occurs(lo)
        case inst => occurs(inst)
      case tp: TypeVar => occurs(tp.underlying)
      case TypeBounds(lo, hi) => occurs(lo) || occurs(hi)
      case _ =>
        val tp1 = tp.dealias
        (tp1 ne tp) && occurs(tp1)
    occurs(inst)
  end occursAtToplevel
}
