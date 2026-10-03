class MaybeInt:
  def isEmpty: Boolean = false
  def get: Int = 1

object extractors:
  object SomeInt:
    def unapply(value: Any): MaybeInt = new MaybeInt

  object Even:
    def unapply(value: Int): Boolean = value % 2 == 0

object SelectedExtractors:
  def option(value: Any): Int =
    value match
      case extractors.SomeInt(x) => x
      case _ => 0

  def boolean(value: Int): Int =
    value match
      case extractors.Even() => 1
      case _ => 0
