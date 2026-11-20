package anchorsearch

trait Key[A]
class Target
object Target:
  given targetKey: Key[Target] = new Key[Target] {}
object Aliases:
  type Public = Target

object Tokens:
  opaque type Token = Int
  def useHere(using ev: Key[Token]): Int = 3
  def inside(using raw: Key[Int]): Int = useHere
  object Token:
    given tokenKey: Key[Token] = new Key[Token] {}

class Client:
  def useAlias(using ev: Key[Aliases.Public]): Int = 1
  def useToken(using ev: Key[Tokens.Token]): Int = 2
  def viaAlias: Int = useAlias
  def viaOpaque: Int = useToken
