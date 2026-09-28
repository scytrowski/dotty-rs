{
  try
    value match
      case A => first
      case _ => second
  catch
    case e => { recover }
  after
}
