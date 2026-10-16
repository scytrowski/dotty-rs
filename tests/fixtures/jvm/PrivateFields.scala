class PrivateFields:
  private val seed: Int = 42
  private val enabled: Boolean = true
  private val disabled: Boolean = false

  def seedValue(): Int = seed
  def enabledValue(): Boolean = enabled
  def disabledValue(): Boolean = disabled
