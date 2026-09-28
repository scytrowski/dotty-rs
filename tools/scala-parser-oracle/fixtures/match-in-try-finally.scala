{
  try
    value match
      case A => first
      case _ => second
  finally cleanup()
  after
}
