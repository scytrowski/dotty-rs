package convergence

trait ChainRoot

class ChainMiddle extends ChainRoot

class ChainLeaf extends ChainMiddle {
  def answer: Int = 42
}
