class Box {
  def unary_! : Box = this
  def unary_~ : Box = this
  def unary_+ : Box = this
  def unary_- : Box = this
}

class Use {
  def not(value: Box): Box = !value
  def complement(value: Box): Box = ~value
  def positive(value: Box): Box = +value
  def negative(value: Box): Box = -value
}

class ValueBox {
  val unary_! : ValueBox = this
}

class BoundaryUse {
  def valueMember(value: ValueBox): ValueBox = !value
}
