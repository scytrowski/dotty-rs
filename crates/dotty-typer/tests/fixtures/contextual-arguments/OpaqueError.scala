package opaqueerror
trait Key[A]
object Tokens:
  opaque type Token = Int
class Client:
  def needs(using k: Key[Tokens.Token]): Int = 1
  def forged(using raw: Key[Int]): Int = needs
