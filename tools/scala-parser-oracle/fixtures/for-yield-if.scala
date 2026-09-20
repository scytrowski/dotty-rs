for (x <- xs) yield if pred(x) then x else fallback
