object Outer {
  private def enumLookupMethods(constraints: Constraints) =
    def scaffolding =
      if constraints.isEnumeration then
        enumScaffolding(constraints.enumCases.map(_._2))
      else
        Nil
    def fromOrdinal =
      if !constraints.cached then
        fromOrdinalMeth(throwArg)
      else
        fromOrdinalMeth(ordinal =>
          Match(ordinal,
            constraints.enumCases.map((i, enumValue) => CaseDef(i, enumValue))
              :+ default(ordinal)))
    if !enumClass.exists then Nil
    else scaffolding ::: valueCtor ::: fromOrdinal :: Nil
  end enumLookupMethods
}
