trait MultilineSelfType {
  this: A & B &
    C =>

  def member = 1
}
