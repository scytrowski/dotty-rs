object InvalidByNameCaseClass:
  case class Node(value: => Int)
  def unrelated: Int = 1
