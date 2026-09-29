case class Setting[T] private[Settings] (
  legacyChoices: Option[Seq[?]] = None)(private[Settings] val idx: Int)(using ct: ClassTag[T]):
  val rendered = idx.toString
