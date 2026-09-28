{
  val result =
    try
      risky()
    catch
      case error: RuntimeException => recover(error)
      case ex: Exception => recover(ex)
  result
}
