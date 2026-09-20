value match { case Foo(x) | Bar(x) => x; case head :: tail => head }
