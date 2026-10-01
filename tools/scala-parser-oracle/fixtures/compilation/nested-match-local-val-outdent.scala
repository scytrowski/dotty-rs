object NestedMatchLocalVal {
  def transform(paramss: List[Int]) = paramss match
    case params :: paramss1 =>
      val (tmap1, params1) = (params match
        case ValDefs(vparams) => transformDefs(vparams)
        case TypeDefs(tparams) => transformDefs(tparams)
      )
      val (tmap2, paramss2) = tmap1.transformAllParamss(paramss1)
      (tmap2, params1 :: paramss2)
    case Nil =>
      (this, paramss)

  def after = 1
}
