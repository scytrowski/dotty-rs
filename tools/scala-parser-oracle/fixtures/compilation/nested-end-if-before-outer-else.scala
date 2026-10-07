trait MessageRendering {
  def messageAndPos(dia: Diagnostic)(using Context): String =
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
    val posString = posStr(pos1, msg, diagnosticLevel(dia))
    if posString.nonEmpty then sb.append(posString).append(EOL)
    if pos.exists && pos1.exists && pos1.source.file.exists then
      val (srcBefore, srcAfter, offset) = sourceLines(pos1)
      val marker = positionMarker(pos1)
      val err = errorMsg(pos1, msg.message, srcAfter.nonEmpty)
      sb.append((srcBefore ::: marker :: err :: srcAfter).mkString(EOL))

      if inlineStack.nonEmpty then
        sb.append(EOL).append(newBox())
        sb.append(EOL).append(offsetBox).append("Inline stack trace")
        for inlinedPos <- inlineStack do
          sb.append(EOL).append(newBox(soft = true))
          sb.append(EOL).append(offsetBox).append("inlined code")
          if inlinedPos.source.file.exists then
            val (srcBefore, srcAfter, _) = sourceLines(inlinedPos)
            val marker = positionMarker(inlinedPos)
            sb.append(EOL).append((srcBefore ::: marker :: srcAfter).mkString(EOL))
        sb.append(EOL).append(endBox)
      end if
    else sb.append(msg.message)
    if dia.isVerbose then
      appendFilterHelp(dia, sb)
}
