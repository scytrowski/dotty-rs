object InvalidContextualLeadingMultipleCaseClass:
  case class Node(using value: Int)(tag: Int)
