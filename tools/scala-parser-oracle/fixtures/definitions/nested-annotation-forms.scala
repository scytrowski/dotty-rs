{
  class CompanionMethodProbe(
      @(`inline` @getter @setter) private var key: Int
  )

  @(deprecated @companionMethod)("replacement", "3.0")
  class CompanionClassProbe
}
