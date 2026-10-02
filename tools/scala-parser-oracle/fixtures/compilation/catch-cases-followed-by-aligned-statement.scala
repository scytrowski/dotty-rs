object CatchCasesFollowedByStatement {
  def process(value: Int) = {
    try value.toString
    catch
      case _: IllegalArgumentException => "bad"
      case _ => "other"
      val after = "done"
      after
  }
}
