(comment) @comment
(string) @string
(number) @number
[
  "main" "fn" "export" "extern" "type" "enum" "property"
  "let" "var" "inout" "return" "if" "else" "for" "in" "while"
] @keyword
(type) @type
(function_declaration name: (identifier) @function)
(extern_declaration name: (identifier) @function)
(property_declaration name: (identifier) @function)
