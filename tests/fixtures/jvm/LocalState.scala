class LocalState {
  def copy(x: Int): Int = {
    val saved = x
    saved
  }

  def overwrite(x: Int): Int = {
    var current = x
    current = 42
    current
  }

  def runOnce(flag: Boolean): Int = {
    var seen = 0
    var running = flag
    while running do {
      seen = 1
      running = false
    }
    seen
  }

  def parameterShadow(value: Int): Int = {
    val value = 42
    value
  }

  def nestedShadow(): Int = {
    val value = 1; {
      val value = 42
      value
    }
    value
  }
}
