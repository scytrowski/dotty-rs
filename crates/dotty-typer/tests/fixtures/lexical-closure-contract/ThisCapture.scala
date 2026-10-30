class Outer:
  def self: Int => Outer = _ => this
  def explicit: Int => Outer = x => Outer.this

trait OuterTrait:
  def selfTrait: Int => OuterTrait = _ => this
