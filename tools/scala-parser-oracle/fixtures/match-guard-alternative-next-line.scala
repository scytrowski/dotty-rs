{
  value match {
    case Left(x) | Right(x)
      if x > 0 => x
    case _ => 0
  }
}
