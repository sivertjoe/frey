/**
 * Tree-sitter grammar for the Frey language.
 *
 * Aimed at syntax highlighting: it parses the real surface syntax (let/struct,
 * generics with `$T`, `#comptime`, defer, pointers, generic calls and struct
 * literals, UFCS method calls, etc.) closely enough to categorize tokens.
 */

const PREC = {
  pipe: 1,
  assign: 2,
  or: 5,
  and: 10,
  bitor: 15,
  bitxor: 20,
  bitand: 25,
  equality: 30,
  comparison: 35,
  shift: 40,
  additive: 45,
  multiplicative: 50,
  cast: 55,
  unary: 60,
  call: 70,
};

// Comma-style separators allow an optional trailing separator.
const sepBy1 = (sep, rule) => seq(rule, repeat(seq(sep, rule)), optional(sep));
const sepBy = (sep, rule) => optional(sepBy1(sep, rule));

module.exports = grammar({
  name: "frey",

  word: ($) => $.identifier,

  extras: ($) => [/\s/, $.line_comment, $.block_comment],

  conflicts: ($) => [
    [$._expression, $.struct_literal],
    [$.binary_expression, $.call_expression],
    [$.dereference_expression, $.binary_expression, $.call_expression],
    [$.unary_expression, $.binary_expression, $.call_expression],
    [$.reference_expression, $.binary_expression, $.call_expression],
    [$._expression, $.type_identifier],
    [$.type_value, $.type_arguments],
    [$.type_identifier, $.generic_application],
    [$._expression, $._statement],
  ],

  rules: {
    source_file: ($) => repeat($._top_level),

    _top_level: ($) => choice($.declaration, $.import),

    import: ($) => seq("import", field("path", $.string_literal), ";"),

    // ---- Comments ----
    line_comment: (_) => token(seq("//", /[^\n]*/)),
    block_comment: (_) =>
      token(seq("/*", /[^*]*\*+([^/*][^*]*\*+)*/, "/")),

    // ---- Declarations ----
    declaration: ($) =>
      seq(
        optional($.comptime_attribute),
        "let",
        field("name", $.identifier),
        optional(seq(":", field("type", $._type))),
        optional(seq("=", field("value", $._expression))),
        ";",
      ),

    comptime_attribute: (_) => seq("#", "comptime"),

    // ---- Types ----
    _type: ($) =>
      choice(
        $.primitive_type,
        $.generic_type,
        $.generic_application,
        $.type_identifier,
        $.pointer_type,
        $.array_type,
        $.function_type,
        $.tuple_type,
      ),

    tuple_type: ($) =>
      seq(
        "(",
        $._type,
        repeat1(seq(",", $._type)),
        optional(","),
        ")",
      ),

    primitive_type: (_) =>
      choice(
        "Int",
        "UInt",
        "Float",
        "i8",
        "i32",
        "i64",
        "u8",
        "u32",
        "u64",
        "f32",
        "f64",
      ),

    generic_type: ($) => seq("$", $.identifier),

    type_identifier: ($) => $.identifier,

    generic_application: ($) =>
      seq(field("name", $.identifier), $.type_arguments),

    // Flat (no nested `<...>`), matching the parser and avoiding ambiguity
    // with comparisons. Tuple/function types inside `<...>` aren't recognised
    // for highlighting — the real parser accepts them.
    type_arguments: ($) =>
      seq(
        "<",
        sepBy1(
          ",",
          choice($.primitive_type, $.generic_type, $.type_identifier),
        ),
        ">",
      ),

    pointer_type: ($) => prec.right(seq("*", $._type)),

    array_type: ($) => seq("[", $._type, ";", $.integer_literal, "]"),

    function_type: ($) =>
      seq("(", sepBy(",", $._type), ")", "->", $._type),

    // ---- Type parameters: `<$K, $V>` ----
    type_parameters: ($) => seq("<", sepBy1(",", $.type_parameter), ">"),
    type_parameter: ($) => seq("$", $.identifier),

    // ---- Expressions ----
    _expression: ($) =>
      choice(
        $.identifier,
        $.integer_literal,
        $.float_literal,
        $.string_literal,
        $.type_value,
        $.function_literal,
        $.extern_function,
        $.struct_definition,
        $.enum_definition,
        $.struct_literal,
        $.array_expression,
        $.tuple_expression,
        $.parenthesized_expression,
        $.block,
        $.if_expression,
        $.while_expression,
        $.match_expression,
        $.unary_expression,
        $.reference_expression,
        $.dereference_expression,
        $.cast_expression,
        $.binary_expression,
        $.pipe_expression,
        $.assignment_expression,
        $.call_expression,
        $.subscript_expression,
        $.field_expression,
        $.tuple_field_expression,
      ),

    // A bare type used in expression position (only meaningful in #comptime).
    type_value: ($) => $.primitive_type,

    parenthesized_expression: ($) => seq("(", $._expression, ")"),

    function_literal: ($) =>
      seq(
        optional($.type_parameters),
        $.parameter_list,
        optional(seq("->", field("return_type", $._type))),
        field("body", $.block),
      ),

    extern_function: ($) =>
      seq(
        "extern",
        optional(field("c_name", $.string_literal)),
        $.extern_param_list,
        optional(seq("->", field("return_type", $._type))),
      ),

    extern_param_list: ($) =>
      seq("(", sepBy(",", choice($.parameter, $.ellipsis)), ")"),
    ellipsis: (_) => "...",

    parameter_list: ($) => seq("(", sepBy(",", $.parameter), ")"),
    parameter: ($) =>
      seq(field("name", $.identifier), ":", field("type", $._type)),

    struct_definition: ($) =>
      seq(
        "struct",
        optional($.type_parameters),
        "{",
        sepBy(",", $.struct_field),
        "}",
      ),
    struct_field: ($) =>
      seq(field("name", $.identifier), ":", field("type", $._type)),

    enum_definition: ($) =>
      seq(
        "enum",
        optional($.type_parameters),
        "{",
        sepBy(",", $.enum_variant),
        "}",
      ),
    enum_variant: ($) =>
      seq(
        field("name", $.identifier),
        optional(seq("(", sepBy1(",", $._type), ")")),
      ),

    struct_literal: ($) =>
      seq(
        field("name", $.identifier),
        optional($.type_arguments),
        "{",
        sepBy(",", $.field_initializer),
        "}",
      ),
    field_initializer: ($) =>
      seq(field("name", $.identifier), ":", field("value", $._expression)),

    array_expression: ($) => seq("[", sepBy(",", $._expression), "]"),

    tuple_expression: ($) =>
      seq(
        "(",
        $._expression,
        repeat1(seq(",", $._expression)),
        optional(","),
        ")",
      ),

    block: ($) =>
      seq("{", repeat($._statement), optional($._expression), "}"),

    if_expression: ($) =>
      prec.right(
        seq(
          "if",
          field("condition", $._expression),
          field("consequence", $.block),
          optional(seq("else", field("alternative", $._expression))),
        ),
      ),

    while_expression: ($) =>
      seq("while", field("condition", $._expression), field("body", $.block)),

    match_expression: ($) =>
      seq(
        "match",
        field("scrutinee", $._expression),
        "{",
        sepBy(",", $.match_arm),
        "}",
      ),
    match_arm: ($) =>
      seq(
        field("pattern", $._pattern),
        "->",
        field("body", $._expression),
      ),
    _pattern: ($) =>
      choice($.wildcard_pattern, $.variant_pattern, $.binding_pattern),
    wildcard_pattern: (_) => "_",
    variant_pattern: ($) =>
      seq(
        field("name", $.identifier),
        "(",
        sepBy(",", $.identifier),
        ")",
      ),
    binding_pattern: ($) => $.identifier,

    unary_expression: ($) =>
      prec(PREC.unary, seq(choice("-", "!"), $._expression)),

    reference_expression: ($) => prec(PREC.unary, seq("&", $._expression)),
    dereference_expression: ($) => prec(PREC.unary, seq("*", $._expression)),

    cast_expression: ($) =>
      prec(PREC.cast, seq($._expression, "as", field("type", $._type))),

    binary_expression: ($) => {
      const ops = [
        ["||", PREC.or],
        ["&&", PREC.and],
        ["|", PREC.bitor],
        ["^", PREC.bitxor],
        ["&", PREC.bitand],
        ["==", PREC.equality],
        ["!=", PREC.equality],
        ["<", PREC.comparison],
        ["<=", PREC.comparison],
        [">", PREC.comparison],
        [">=", PREC.comparison],
        ["<<", PREC.shift],
        [">>", PREC.shift],
        ["+", PREC.additive],
        ["-", PREC.additive],
        ["*", PREC.multiplicative],
        ["/", PREC.multiplicative],
        ["%", PREC.multiplicative],
      ];
      return choice(
        ...ops.map(([op, p]) =>
          prec.left(
            p,
            seq(
              field("left", $._expression),
              field("operator", op),
              field("right", $._expression),
            ),
          ),
        ),
      );
    },

    pipe_expression: ($) =>
      prec.left(PREC.pipe, seq($._expression, "|>", $._expression)),

    assignment_expression: ($) =>
      prec.right(
        PREC.assign,
        seq(field("target", $._expression), "=", field("value", $._expression)),
      ),

    call_expression: ($) =>
      prec(
        PREC.call,
        seq(
          field("function", $._expression),
          optional($.type_arguments),
          $.arguments,
        ),
      ),
    arguments: ($) => seq("(", sepBy(",", $._expression), ")"),

    subscript_expression: ($) =>
      prec(PREC.call, seq($._expression, "[", $._expression, "]")),

    field_expression: ($) =>
      prec(PREC.call, seq($._expression, ".", field("field", $.identifier))),

    tuple_field_expression: ($) =>
      prec(PREC.call, seq($._expression, ".", field("index", $.integer_literal))),

    // ---- Statements ----
    _statement: ($) =>
      choice(
        $.declaration,
        $.return_statement,
        $.break_statement,
        $.defer_statement,
        $.expression_statement,
        // Block-like expressions stand as statements without a `;`.
        $.if_expression,
        $.while_expression,
        $.block,
      ),

    return_statement: ($) => seq("return", optional($._expression), ";"),
    break_statement: (_) => seq("break", ";"),
    defer_statement: ($) => seq("defer", $._expression, ";"),
    expression_statement: ($) => seq($._expression, ";"),

    // ---- Literals & identifiers ----
    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,
    integer_literal: (_) => /\d+/,
    float_literal: (_) => /\d+\.\d+([eE][+-]?\d+)?|\d+[eE][+-]?\d+/,
    string_literal: ($) =>
      seq('"', repeat(choice($.escape_sequence, /[^"\\]/)), '"'),
    escape_sequence: (_) => /\\./,
  },
});
