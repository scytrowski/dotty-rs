package legacyconversions

import scala.language.implicitConversions

class Ambiguous:
  implicit def first(x: From): To = new To
  implicit def second(x: From): To = new To
  def result(x: From): To = x
