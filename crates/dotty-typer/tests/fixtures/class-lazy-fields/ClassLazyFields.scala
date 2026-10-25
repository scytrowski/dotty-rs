class InferredLazyClassFieldForwardReference {
  lazy val forwardFirst = forwardLater
  lazy val forwardLater = 1
}

class InferredLazyClassFieldSelfCycle {
  lazy val selfValue = selfValue
}

class InferredLazyClassFieldMutualCycle {
  lazy val mutualFirst = mutualSecond
  lazy val mutualSecond = mutualFirst
}

class ExplicitLazyClassFieldSelfReference {
  lazy val typedValue: Int = typedValue
}
