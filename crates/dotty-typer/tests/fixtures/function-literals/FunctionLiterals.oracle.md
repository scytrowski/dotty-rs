# Explicitly typed function literal oracle

`FunctionLiterals.scala` covers zero, one, and multiple explicitly typed
parameters, a block body, and a lambda passed to an ordinary method. Its
reference output is recorded in `FunctionLiterals.scala39-typed-tree.txt` using
the pinned Scala 3.9.0 compiler artifact and `-Vprint:typer,lambdaLift
-Xprint-types -Yprint-pos -Yprint-pos-syms`.

The source compiler revision is
`777528f19a58e794c9954a42f433373472ec57f8`. At the end of `typer`, each lambda
is a typed function literal with its explicit parameter types, body result,
and canonical `FunctionN` type. In `lambdaLift`, each source lambda becomes a
`Closure` whose `method` references a synthetic `$init$$$anonfun$N` method;
that method owns the lambda parameters and typed body. The fixture's lambdas
capture no values, so the closure environment is empty. Lambda, closure, and
synthetic method positions point back to the corresponding source expression;
the generated method name is compiler-owned and is not a source lookup name.

The relevant canonical types are `Function0[Int]`, `Function1[Int, Int]`, and
`Function2[Int, Int, Int]`. A lambda passed to `consume` has the same closure
shape as one assigned to a value. The implementation in this issue emits the
shared `Closure` representation at the source typing boundary and gives each
lambda a typer-owned synthetic method identity; it does not attempt Scala's
later JVM specialization or bytecode lowering.
