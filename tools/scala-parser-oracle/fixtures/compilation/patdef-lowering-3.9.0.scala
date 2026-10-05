object PatDefLoweringOracle:
  def pair(): (Int, String) = (1, "one")
  def option(): Option[(Int, String)] = Some(pair())

  def oneBinder(value: Option[String]): String =
    val Some(x) = value
    x

  def zeroBinders(): Unit =
    val (_, _) = pair()
    ()

  def multipleBinders(value: Option[(Int, String)]): (Int, String) =
    val Some((a, b)) = value
    (a, b)

  def tupleBinders(): (Int, String) =
    val (a, b) = pair()
    (a, b)

  def aliasedBinder(value: Option[(Int, String)]): Option[(Int, String)] =
    val whole @ Some(_) = value
    whole

  def mutableBinders(): Int =
    var (a, b) = pair()
    a = 2
    a + b.length

  def lazyBinders(): Int =
    lazy val (a, b) = pair()
    a + b.length

  def annotatedBinders(): (Int, String) =
    val (a, b): (Int, String) = pair()
    (a, b)

  def uncheckedTuple(value: (Any, Any)): String =
    val (item: String, _) = value: @unchecked
    item

  def runtimeCheckedTuple(value: (Any, Any)): String =
    val (item: String, _) = value.runtimeChecked
    item
