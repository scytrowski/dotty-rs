package contextualextensions

class Ambiguous:
  def choose(box: Box)(using first: Ops, second: Ops, fmt: Format): Int = box.render(3)
