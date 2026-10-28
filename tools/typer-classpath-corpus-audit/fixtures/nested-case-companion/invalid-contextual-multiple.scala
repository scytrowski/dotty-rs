object InvalidContextualMultipleCaseClass:
  case class Node(using value: Int)(using tag: Int)
