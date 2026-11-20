package trailingviews

class Rejected extends Client {
  def missingMember(x: Box[Int]): Int = x.show
  def missingArgument(x: Box[Int]): Int = accept(x)
  def ambiguousMember(x: Parcel)(using first: TC[Int], second: TC[Int]): Int = x.show
  def ambiguousArgument(x: Parcel)(using first: TC[Int], second: TC[Int]): Int = accept(x)
}
