package contextualconversions

class Conflict:
  def result(x: From)(using first: Conversion[From, To], second: Conversion[From, To]): To = x
