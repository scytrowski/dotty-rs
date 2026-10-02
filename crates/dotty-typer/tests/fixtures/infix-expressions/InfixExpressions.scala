class Box { def combine(other: Box): Box = this }
class Use {
  def infix(left: Box, right: Box): Box = left `combine` right
  def direct(left: Box, right: Box): Box = left.combine(right)
}
