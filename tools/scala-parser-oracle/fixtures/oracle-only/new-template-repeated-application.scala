package repro

object RepeatedNewApplication:
  def values =
    new Foo {}(1)
    (2, 3)
