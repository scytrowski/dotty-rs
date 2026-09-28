{
  class C:
    def first =
      new URL(null, "memory", new URLStreamHandler {
        override def openConnection(url: URL): URLConnection = new URLConnection(url) {
          override def connect() = ()
          override def getInputStream = 1
        }
      })

  class D:
    def factory = new Factory(null, new Members {
      private def privateMember = 1
      given Service = service
      override def overridden = 2
      inline def inlined = 3
    })
}
