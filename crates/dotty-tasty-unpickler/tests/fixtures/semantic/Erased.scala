package me.cytrowski.tastyfixtures.semantic

// Real erased parameters, to record how Scala 3.9 writes their type. A method's
// own type is not pickled; a function type and a lambda's method type are.
import scala.language.experimental.erasedDefinitions

class ErasedParams:
  def proof(erased x: Int, y: Int): Int = y
  val lambdaMember: (erased Int) => Int = (erased z: Int) => 0
  def local(): Int =
    val f = (erased z: Int, w: Int) => w
    0
