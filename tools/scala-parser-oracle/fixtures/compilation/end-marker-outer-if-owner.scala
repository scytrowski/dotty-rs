object EndMarkerOuterIfOwner {
  def value(outer: Boolean, inner: Boolean) =
    if outer then
      if inner then 1
    end if
}
