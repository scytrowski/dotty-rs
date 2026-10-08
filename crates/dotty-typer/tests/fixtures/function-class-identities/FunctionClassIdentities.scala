object FunctionClassIdentities:
  type Zero = () => Int
  type One = Int => String
  type Many = (Int, String) => Boolean
  type Contextual = String ?=> Int
