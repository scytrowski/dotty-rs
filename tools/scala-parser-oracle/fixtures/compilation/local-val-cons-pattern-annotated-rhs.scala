object LocalValConsPatternAnnotatedRhs:
  def split(cls: ClassInfo) =
    val firstParent :: otherParents = cls.info.parents: @unchecked
    (firstParent, otherParents)
