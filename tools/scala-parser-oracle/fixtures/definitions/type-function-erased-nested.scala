{
  import scala.language.experimental.erasedDefinitions
  val f: (erased x: A) => (y: B) => C = value
  f
}
