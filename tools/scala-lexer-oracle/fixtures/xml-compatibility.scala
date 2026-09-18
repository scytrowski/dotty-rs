val spaced = <root attr = "value" enabled = {flag} />
val qualified = <ns:root.child data-id = "value"><child-node /></ns:root.child>
val text = <root>text &amp; more < nested </root>
val cdata = <root><![CDATA[<not><xml/>]]></root>
val comment = <root><!-- <nested/> --><child /></root>
val nestedExpression = <root>{if condition then <yes/> else <no/>}</root>
val nestedBraces = <root>{{value}}</root>
