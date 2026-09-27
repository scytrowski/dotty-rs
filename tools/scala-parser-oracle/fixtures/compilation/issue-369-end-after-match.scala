class C {
  def f = value match
    case A =>
      first
    case _ =>
      second
  end f
}
