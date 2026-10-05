def f(value: Any) =
  value: @unchecked match
    case _ => value
