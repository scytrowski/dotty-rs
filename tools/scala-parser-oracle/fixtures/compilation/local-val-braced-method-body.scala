object LocalValBracedMethodBody {
  def split(values: List[Int]): (Int, List[Int]) =
    val evidenceNames = ListBuffer.empty[Int]
    val firstParent :: otherParents = values: @unchecked
    (firstParent, otherParents)
}
