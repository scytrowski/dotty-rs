object SimpleExtractor {
  def unapply(value: Any): Option[Int] = Some(1)
}

object ExtractorFoundation {
  def choose(value: Any): Int = value match {
    case SimpleExtractor(_) => 1
    case _ => 0
  }
}
