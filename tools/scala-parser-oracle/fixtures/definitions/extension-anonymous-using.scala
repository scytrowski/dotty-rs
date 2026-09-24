{
  extension (using Context)(value: Value) def f = value
  extension (value: Value)(using Context) def g = value
}
