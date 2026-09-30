object SelectorMatch:
  def run(value: Option[Int]) =
    value.match
      case Some(x) => x
      case None => 0
    end match
