trait InvalidComposeFunction[-T1, +R]:
  def apply(v1: T1): R

  def wrongArgument[A](g: A => T1): A => R =
    x => apply(g(1))

  def wrongOwner[A](g: String => T1): A => R =
    x => apply(g(x))

  def wrongVariance[A](g: Nothing => T1): A => R =
    x => apply(g(x))

  def wrongArity[A](g: A => T1): A => R =
    x => apply(g(x, x))

class Unrelated[A, T1, R]:
  def apply(value: String): T1

trait UnrelatedApplyControl[T1, R]:
  def apply(v1: T1): R

  def unrelatedApply[A](g: Unrelated[A, T1, R]): A => R =
    x => apply(g(x))
