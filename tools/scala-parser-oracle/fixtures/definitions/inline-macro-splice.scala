{
  inline def make: Int = ${ makeImpl }
  inline def nested(value: Int): Int = ${ wrap(makeImpl(value + 1)) }
  inline def quoted(value: Int): Int = ${ makeImpl('value) }
}
