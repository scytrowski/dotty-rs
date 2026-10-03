object LazyBodyAnnotation {
  def apply(bodyFn: Context ?=> Tree): LazyBodyAnnotation =
    new LazyBodyAnnotation:
      protected var myTree: Tree | (Context ?=> Tree) | Null = ctx ?=> bodyFn(using ctx)
}
