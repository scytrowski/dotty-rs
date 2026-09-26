{
  value match {
    case Some(x)

      if x > 0 => x
    case _ => 0
  }
}
