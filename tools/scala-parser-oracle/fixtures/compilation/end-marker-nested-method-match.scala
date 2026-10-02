object EndMarkerNestedMethodMatch {
  def outer =
    def inner(value: Int) =
      value match
        case 0 => 0
        case other => other
    inner(1)
  end outer
}
