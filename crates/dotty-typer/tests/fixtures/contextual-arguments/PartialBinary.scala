package partialbinary

// The binary fixture omits Unavailable so the provider has a partial signature.
class Unavailable
trait PartialTC[A] extends structural.TC[A]
object Providers:
  given partialTC: PartialTC[Unavailable] = ???
