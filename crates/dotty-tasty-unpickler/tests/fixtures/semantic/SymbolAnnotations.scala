package me.cytrowski.tastyfixtures.semantic

// Serialized symbol annotations (Milestone 5e1): `ANNOTATION` tail entries on
// a definition or parameter, as opposed to `ANNOTATEDtype`/`ANNOTATEDtpt`
// (Annotations.scala), which annotate a type or type tree instead.
class SymbolMarker extends scala.annotation.StaticAnnotation
class SymbolTagged(tag: String) extends scala.annotation.StaticAnnotation

@SymbolMarker class SymbolAnnotated:
  @SymbolMarker val x: Int = 1
  @SymbolMarker def f(x: Int): Int = x
  @SymbolMarker type T = String

class SymbolAnnotatedParams(@SymbolMarker val ctorParam: Int):
  def method[@SymbolMarker A](@SymbolMarker @SymbolTagged(tag = "p") param: A): A = param

class SymbolUnannotated:
  val plain: Int = 2
