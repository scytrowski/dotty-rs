package me.cytrowski.tastyfixtures.semantic

import scala.annotation.StaticAnnotation

// Milestone 5b: selected, singleton and annotated type trees. Everything a
// tree refers to is nested in the one holder, so it is a same-unit symbol.

class SelectedHolder:
  trait Box:
    type Out

  object Stable

  class Marker extends StaticAnnotation

  val box: Box = ???
  type Selected = box.Out
  val stable: Stable.type = Stable
  type SingletonAlias = stable.type
  val annotated: Int @Marker = 1
  type AnnotatedAlias = Any @Marker
  def use(x: Box): x.Out = ???
