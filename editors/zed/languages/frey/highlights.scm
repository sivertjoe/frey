; Comments
[
  (line_comment)
  (block_comment)
] @comment

; The `#comptime` directive
(comptime_attribute) @keyword

; Declaration names: struct types, functions, then plain variables.
; (Earlier patterns win, so the specific ones come first.)
(declaration
  name: (identifier) @type
  value: (struct_definition))

(declaration
  name: (identifier) @function
  value: (function_literal))

(declaration
  name: (identifier) @variable)

; Parameters
(parameter
  name: (identifier) @variable.parameter)

; Calls and UFCS method calls
(call_expression
  function: (identifier) @function)

(call_expression
  function: (field_expression
    field: (identifier) @function.method))

; Struct construction
(struct_literal
  name: (identifier) @type)
(generic_application
  name: (identifier) @type)

; Struct fields and field access
(struct_field
  name: (identifier) @property)
(field_initializer
  name: (identifier) @property)
(field_expression
  field: (identifier) @property)

; Types
(primitive_type) @type.builtin
(type_value) @type.builtin
(type_identifier) @type
(generic_type) @type
(type_parameter) @type

; Keywords
[
  "let"
  "mut"
  "struct"
  "as"
  "if"
  "else"
  "while"
  "return"
  "break"
  "defer"
] @keyword

; Literals
(integer_literal) @number
(float_literal) @number
(string_literal) @string
(escape_sequence) @string.escape

; Operators
[
  "+"
  "-"
  "*"
  "/"
  "%"
  "<"
  ">"
  "<="
  ">="
  "=="
  "!="
  "<<"
  ">>"
  "&"
  "|"
  "^"
  "&&"
  "||"
  "!"
  "="
  "|>"
  "->"
] @operator

; Punctuation
[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

[
  ","
  ";"
  ":"
  "."
] @punctuation.delimiter
