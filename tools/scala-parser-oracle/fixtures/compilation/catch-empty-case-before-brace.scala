object EmptyCatchBeforeBrace:
  def f = {
    try risky()
    catch
      case _: RuntimeException =>
  }
