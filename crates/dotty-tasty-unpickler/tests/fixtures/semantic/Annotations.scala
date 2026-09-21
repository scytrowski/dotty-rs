package me.cytrowski.tastyfixtures.semantic

// Full annotation trees. The compiler writes an `ANNOTATEDtype` only where a
// type (not a type tree) is pickled, such as the inferred type of a definition.
class Label(val label: String) extends scala.annotation.StaticAnnotation
class Tag extends scala.annotation.StaticAnnotation

class Annotated:
  def inferred = (1: Int @Tag)
  def body(): Int =
    val local = (1: Int @Tag @Label("second"))
    local
