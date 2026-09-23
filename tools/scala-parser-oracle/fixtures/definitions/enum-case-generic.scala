{
  enum Box:
    case Packed[A](value: A)
    case Pair[A, B](left: A, right: B)
}
