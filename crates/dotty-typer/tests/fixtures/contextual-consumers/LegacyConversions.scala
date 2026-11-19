package legacyconversions

import scala.language.implicitConversions

class From
class To
class Client:
  implicit def old(x: From): To = new To
  def oldOnly(x: From): To = x
  def nearest(x: From)(using inner: Conversion[From, To]): To = x

class Mixed:
  given instance: Conversion[From, To] = ???
  implicit def old(x: From): To = new To
  def mixed(x: From): To = x
