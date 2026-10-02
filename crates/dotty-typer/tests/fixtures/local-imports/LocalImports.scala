package lib {
  object Owner {
    val member: Int = 1
    class Box
  }
}

package app {
  object Use {
    def term: Int = {
      import lib.Owner.member
      member
    }

    def localType(value: lib.Owner.Box): lib.Owner.Box = {
      import lib.Owner.Box
      val box: Box = value
      box
    }
  }
}
