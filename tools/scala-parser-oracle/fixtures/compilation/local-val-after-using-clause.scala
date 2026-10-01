object LocalValAfterUsingClause {
  private def desugarContextBounds(
      allParamss: List[ParamClause])(using Context): TypeDef =
    val evidenceNames = ListBuffer.empty[TermName]
    evidenceNames
}
