package contextualextensions

class Box
trait Format
trait Ops:
  extension (box: Box) def render(index: Int)(using fmt: Format): Int

  extension (box: Box) def size(using fmt: Format): Int

class Client(using outer: Ops):
  def choose(box: Box)(using inner: Ops, fmt: Format): Int = box.render(3)
  def property(box: Box)(using inner: Ops, fmt: Format): Int = box.size
