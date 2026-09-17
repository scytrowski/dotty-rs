val simple = <message><body>hello</body></message>
val nested = <root>{value}</root>
val selfClosing = <tag/>
val nestedSelfClosing = <root><child/></root>
val expression = <root>{a < b}</root>
val nestedExpression = <root>{<inner/>}</root>
val attributes = <item id="x" enabled={flag}>text</item>
val comment = <root><!-- comment --><x><![CDATA[text]]></x></root>
val spaced = a < b
val tight = a <b
