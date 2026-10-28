object InvalidContextualOnlyCaseClass:
  case class Node(using value: Int)
  def unrelated: Int = 1
