{
  if cond then
    value match
      case A => first
      case _ => second
  else fallback
  after
}
