{
  for x <- xs yield
    val result = x
    transform(result)
  after
}
