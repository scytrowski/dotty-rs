object Test:
  def choose(flag: Boolean): Int = {
    if flag then
      1;
      else 0
  }
  val after = choose(true)
