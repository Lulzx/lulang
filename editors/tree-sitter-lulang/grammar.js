module.exports = grammar({
  name: "lulang",
  extras: $ => [/\s/, $.comment],
  word: $ => $.identifier,
  rules: {
    source_file: $ => repeat(choice($.declaration, $.main_block)),
    comment: _ => token(seq("//", /.*/)),
    identifier: _ => /[A-Za-z_][A-Za-z0-9_]*/,
    number: _ => /\d+(\.\d+)?([eE][+-]?\d+)?/,
    string: _ => seq('"', repeat(choice(/[^"\\]/, /\\./)), '"'),
    type: $ => choice("i32", "i64", "f32", "f64", "bool", "str", "()",
      seq(choice("c_ptr", "c_slice"), "[", $.type, "]"),
      $.identifier, seq("[", $.type, "]")),
    declaration: $ => choice($.function_declaration, $.extern_declaration,
      $.type_declaration, $.enum_declaration, $.property_declaration),
    parameters: $ => seq("(", optional(commaSep($.parameter)), ")"),
    parameter: $ => seq(optional("inout"), $.identifier, ":", $.type),
    function_declaration: $ => seq(optional("export"), "fn", field("name", $.identifier),
      $.parameters, optional(seq(":", $.type)), $.block),
    extern_declaration: $ => seq("extern", optional($.string), "fn",
      field("name", $.identifier), $.parameters, optional(seq(":", $.type))),
    type_declaration: $ => seq("type", field("name", $.identifier), "{",
      optional(commaSep(seq($.identifier, ":", $.type))), "}"),
    enum_declaration: $ => seq("enum", field("name", $.identifier), "{",
      optional(commaSep($.identifier)), "}"),
    property_declaration: $ => seq("property", field("name", $.identifier),
      $.parameters, $.block),
    main_block: $ => seq("main", $.block),
    block: $ => seq("{", repeat($.statement), "}"),
    statement: $ => choice(
      seq(choice("let", "var"), $.identifier, "=", $.expression),
      seq("return", optional($.expression)),
      seq("if", $.expression, $.block, optional(seq("else", $.block))),
      seq("while", $.expression, $.block),
      seq("for", $.identifier, "in", $.expression, "..", $.expression, $.block),
      seq($.expression, optional(seq("=", $.expression)))
    ),
    expression: $ => choice(
      $.identifier, $.number, $.string, "true", "false",
      seq("(", $.expression, ")"),
      prec.left(1, seq($.expression, choice("+", "-", "*", "/", "%", "==",
        "!=", "~=", "<", "<=", ">", ">=", "and", "or"), $.expression)),
      prec(2, seq(choice("-", "not"), $.expression)),
      prec(3, seq($.expression, "(", optional(commaSep($.expression)), ")")),
      prec(3, seq($.expression, "[", $.expression, "]")),
      prec(3, seq($.expression, ".", $.identifier))
    )
  }
});

function commaSep(rule) {
  return seq(rule, repeat(seq(",", rule)), optional(","));
}
