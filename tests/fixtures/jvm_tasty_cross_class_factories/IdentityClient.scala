package client

import example.{Counter, CounterIdentity}

object IdentityClient:
  def main(args: Array[String]): Unit =
    val counter = new Counter(7)
    assert(new CounterIdentity().identity(counter) eq counter)
