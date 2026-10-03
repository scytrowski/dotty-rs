object MatchEmptyNestedCaseBeforeFollowingStatements {
  def checkValid(mt: MethodType): MethodType = {
    var traverser = new TypeTraverser:
      val params = mt.paramNames.zip(mt.paramRefs).toMap
      def traverse(t: Type) =
        t match
          case CapturingType(parent, refs) =>
            def checkRefs(refs: CaptureSet) =
              for elem <- refs.elems do
                elem match
                  case elem: TermParamRef =>
                    val elemName = elem.binder.paramNames(elem.paramNum)
                    params.get(elemName) match
                      case Some(elemRef) => assert(elemRef eq elem, mt)
                      case _ =>
                  case ResultCap(binder: MethodType) if binder ne mt =>
                    assert(binder.paramNames.toList != mt.paramNames.toList, mt)
                  case _ =>
              checkRefs(refs)
              traverse(parent)
          case _ =>
            traverseChildren(t)
    traverser.traverse(mt.resType)
    mt
  }
}
