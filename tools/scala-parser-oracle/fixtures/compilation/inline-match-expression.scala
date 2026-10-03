inline def classify(inline value: Any) = inline value match
  case _: Int => 1
  case _ => 2
