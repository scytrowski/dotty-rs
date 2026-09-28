object X {
  def init = {
    def addPrimitive(s: S, code: Int): Unit = primitives(s) = code

    def addPrimitives(cls: S, method: M, code: Int)(using Context): Unit = {
      val alts = cls.info.member(method).alternatives.map(_.symbol)
      if (alts.isEmpty)
        report.error(em"Unknown primitive method $cls.$method")
      else alts foreach (s =>
        addPrimitive(s,
          s.info.paramInfoss match {
            case List(tp :: _) => CONCAT
            case _                                          => code
          }
        )
        )
    }
  }
}
