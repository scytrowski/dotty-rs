trait MessageRendering {
  def messageAndPos(dia: Diagnostic)(using Context): String =
    def adjust(pos: SourcePosition): SourcePosition =
      if pos.span.isSynthetic
      && pos.span.isZeroExtent
      && pos.span.exists
      && pos.span.start == pos.source.length
      && pos.source(pos.span.start - 1) == '\n'
      then
        pos.withSpan(pos.span.shift(-1))
      else
        pos
    val msg = dia.msg
    val pos = dia.pos
    val pos1 = adjust(pos.nonInlined)
    val outermost = pos.outermost
    val inlineStack = pos.inlinePosStack.filterNot(outermost.contains(_))
    given Level = Level(dia.level)
    given Offset =
      val maxLineNumber =
        if pos.exists then (pos1 :: inlineStack).map(_.endLine).max + 1
        else 0
      Offset(maxLineNumber.toString.length + 2)
    val sb = StringBuilder()
    sb.toString

  def later: Int = 1
}
