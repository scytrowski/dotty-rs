(
  value match {
    case Some(x) =>
      first(x)
      second(x)
    case _ => done()
  }
)
