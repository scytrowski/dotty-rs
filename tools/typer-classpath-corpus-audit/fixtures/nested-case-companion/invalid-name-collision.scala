object InvalidCaseClassNameCollision:
  def Node: Int = 1
  case class Node(value: Int)
