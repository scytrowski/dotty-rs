{
  type Result = T match {
    case List[x] => x
    case h *: t => h
    case (a, b) => a
    case _ => Any
  }
}
