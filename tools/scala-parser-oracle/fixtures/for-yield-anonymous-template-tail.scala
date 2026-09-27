{
  for (x, y) <- values yield
    val cleanup = new C:
      def transform = item match
        case A => first
        case _ => second
    (x, y)
}
