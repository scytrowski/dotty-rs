package demo

package object foo {
  @deprecated("old", "3.0")
  def answer = 42
}
