{
  import scala.language.experimental.erasedDefinitions
  type F = (erased A) ?=> B => C
  type G = (erased A) => B ?=> C
  F
  G
}
