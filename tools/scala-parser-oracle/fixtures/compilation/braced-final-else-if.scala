object BracedFinalElseIf {
  def result(x: Boolean, y: Boolean, z: Boolean): Int = {
    if x then
      if y then 1
    else if z then 3
    else 4
  }
}
