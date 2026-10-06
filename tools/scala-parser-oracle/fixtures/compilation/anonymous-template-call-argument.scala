object Inferencing:
  def instantiateSelected(tp: Type, tvars: List[Type]): Unit =
    IsFullyDefinedAccumulator(
      new ForceDegree.Value(IfBottom.flip):
        override def appliesTo(tvar: TypeVar) = tvars.contains(tvar),
      minimizeSelected = true
    ).process(tp)

  def next = 2
