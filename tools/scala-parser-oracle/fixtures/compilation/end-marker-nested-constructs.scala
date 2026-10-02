object EndMarkerNestedConstructs {
  def result(value: Int) =
    value match
      case 0 => 0
      case n => n
    end match
  end result
}
