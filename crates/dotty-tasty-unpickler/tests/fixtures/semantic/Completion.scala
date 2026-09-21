package me.cytrowski.tastyfixtures.semantic

// Milestone 5a: simple symbol completion. Every definition below has a
// declared type tree; `stable.Out` and `x.Out` are the term paths whose prefix
// is completed first.

type CompletionAlias = Int
type CompletionAbstract <: Any
val completionPlain: Int = 1

class CompletionHolder[A](val field: A):
  trait CBox:
    type Out

  val plain: Int = 1
  val generic: List[Int] = Nil
  val stable: CBox = ???
  var mutable: CBox = ???
  type Selected = stable.Out
  type Alias = Int
  type Abstract <: Any
  type Bounded >: Nothing <: CBox
  def use(x: CBox): x.Out = ???
  def byName(x: => CBox): Int = 1
