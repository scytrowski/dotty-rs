// A unit with no `package` clause: it lives in the default (empty) package.
// `Widget.Inner` is a member class, and `use` refers to a class of the same
// unit, so both an owner chain and references can be checked.
class Widget {
  class Inner
  def make: Inner = new Inner
}

object Holder {
  val one: Int = 1
  def use(p: Widget): Widget = p
}
