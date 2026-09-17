val simple = <message><body>hello</body></message>
val nested = <root>{value}</root>
val selfClosing = <tag/>
val nestedSelfClosing = <root><child/></root>
val expression = <root>{a < b}</root>
val nestedExpression = <root>{<inner/>}</root>
val spaced = a < b
val tight = a <b
