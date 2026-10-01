object LocalValConsPattern:
  def split(parents: List[Int]) =
    val firstParent :: otherParents = parents
    (firstParent, otherParents)
