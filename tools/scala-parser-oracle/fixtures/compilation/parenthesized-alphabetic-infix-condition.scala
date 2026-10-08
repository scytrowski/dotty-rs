object ParenthesizedInfixCondition:
  def alpha(left: AnyRef, right: AnyRef) =
    if (left.asInstanceOf[AnyRef]) eq (right.asInstanceOf[AnyRef]) then true else false

  def parenthesized(value: Boolean) =
    if (value) true else false
