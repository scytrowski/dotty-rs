package matching
class Box(val value: Int)
object NonCaseMissingExtractor:
  def extract(box: Box): Int = box match
    case Box(value) => value
