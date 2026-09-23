{
  import scala.language.experimental.erasedDefinitions
  val f: (erased x: A, erased y: B) => C = value
  f
}
