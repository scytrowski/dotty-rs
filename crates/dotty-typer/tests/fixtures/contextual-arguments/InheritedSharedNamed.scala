package inheritexport
object SharedNamed extends Base[Int]:
  export Ints.{key as cloned}
