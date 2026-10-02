object EndMarkerNestedMatch {
  def value(x: Int) = {
    x match
    case 0 => 0
    case n => n
    end match
  }
}
