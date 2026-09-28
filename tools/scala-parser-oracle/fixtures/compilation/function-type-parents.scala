abstract class PredicateParent extends (A => Boolean) with Serializable

abstract class NullablePredicateParent
    extends ((A | Null) => Boolean | Null)
    with Serializable
