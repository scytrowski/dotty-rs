{
  import scala.language.experimental.erasedDefinitions
  val f: (erased x: A) ?=> Box[B] = value
  f
}
