object IfElseAfterMatchThen {
  def hasInnerErrors(t: Tree, argType: Type): Boolean =
    t.existsSubTree { t1 =>
      if t1.typeOpt.isError
        && t.span.toSynthetic != t1.span.toSynthetic
        && t.typeOpt != t1.typeOpt then
        println(t1)
        t1.typeOpt match
          case errorType: ErrorType if errorType.msg.isInstanceOf[TypeMismatchMsg] =>
            val mismatch = errorType.msg
            argType != mismatch.expected
          case _ => true
      else false
    }
}
