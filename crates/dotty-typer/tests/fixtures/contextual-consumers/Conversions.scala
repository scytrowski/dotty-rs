package contextualconversions

class From
class To
class Client(using outer: Conversion[From, To]):
  def accept(to: To): To = to
  def result(x: From)(using inner: Conversion[From, To]): To = x
  def argument(x: From)(using inner: Conversion[From, To]): To = accept(x)
