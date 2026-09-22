package me.cytrowski.tastyfixtures.semantic

// Milestone 5d2a: constructor completion. Every shape `normalizeIfConstructor`
// and the effective owner-class result need to see at least once.
//
// `Ord` stands in for `scala.math.Ordering`: the library's own reaches
// `scala.math` through a nested `TERMREF(math, TERMREFpkg(scala))`, not a
// single qualified `TERMREFpkg`, which is a distinct (pre-existing) gap in
// name-based member lookup, unrelated to constructor completion; a
// locally-defined trait keeps these fixtures exercising only 5d2a.
trait CtorOrd[A]

class CtorEmpty

class CtorPlain(x: Int)

class CtorWithVal(val x: Int)

class CtorGeneric[A](x: A)

class CtorContextOnly(using ord: CtorOrd[Int])

class CtorOldImplicit(implicit ord: CtorOrd[Int])

class CtorCurriedGeneric[A](x: A)(using ord: CtorOrd[A])

class CtorGenericContextOnly[A](using ord: CtorOrd[A])

class CtorTwoTermClauses(x: Int)(using y: CtorOrd[Int])

object CtorObj

class CtorOuter:
  class CtorInner[A](x: A)
