; Luau, for tree-sitter-highlight.
;
; Adapted from tree-sitter-grammars/tree-sitter-luau's
; queries/highlights.scm (MIT), which is written for Neovim. As there,
; the last pattern to match a node wins, so the broad patterns come
; first and the narrow ones after. Unlike there:
;
; - Neovim's `#lua-match?` is `#match?` here; tree-sitter ignores the
;   former, which made every name a type and a constant.
; - Its `(type (identifier))` relies on supertype parents, which the
;   grammar's trees do not have here; it matched every identifier.
;   Types are found by their place instead: after a name, a parameter,
;   a parameter list or `::`, and inside a type.

; Names
((identifier) @type
  (#match? @type "^[A-Z]"))

((identifier) @constant
  (#match? @constant "^[A-Z][A-Z_0-9]+$"))

((identifier) @variable.builtin
  (#eq? @variable.builtin "self"))

((identifier) @constant.builtin
  (#eq? @constant.builtin "_VERSION"))

((identifier) @module.builtin
  (#any-of? @module.builtin
    "_G" "bit32" "buffer" "coroutine" "debug" "math" "os" "string" "table"
    "utf8" "vector"))

; Tables
(field
  name: (identifier) @property)

(dot_index_expression
  field: (identifier) @property)

(object_type
  (identifier) @property)

; Functions
(parameter
  .
  (identifier) @variable.parameter)

(function_declaration
  name: (identifier) @function)

(function_declaration
  name: (dot_index_expression
    field: (identifier) @function))

(function_declaration
  name: (method_index_expression
    method: (identifier) @function))

(function_call
  name: (identifier) @function.call)

(function_call
  name: (dot_index_expression
    field: (identifier) @function.call))

(method_index_expression
  method: (identifier) @function.method.call)

(function_call
  name: (identifier) @function.builtin
  (#any-of? @function.builtin
    "assert" "collectgarbage" "error" "gcinfo" "getfenv" "getmetatable"
    "ipairs" "loadstring" "newproxy" "next" "pairs" "pcall" "print"
    "rawequal" "rawget" "rawlen" "rawset" "require" "select" "setfenv"
    "setmetatable" "tonumber" "tostring" "type" "typeof" "unpack" "xpcall"))

; Types
(builtin_type) @type.builtin

(variable_list
  name: (identifier)
  .
  (identifier) @type)

(parameter
  (identifier)
  .
  (identifier) @type)

(function_declaration
  parameters: (_)
  .
  (identifier) @type)

(function_definition
  parameters: (_)
  .
  (identifier) @type)

(cast_expression
  (identifier) @type .)

(type_definition
  name: (_) @type)

(type_definition
  "="
  .
  (identifier) @type)

([
  (generic_type)
  (function_type)
  (optional_type)
  (union_type)
  (intersection_type)
  (variadic_type)
] (identifier) @type)

(comment) @comment

(hash_bang_line) @keyword

; Literals
(escape_sequence) @string.escape

(string) @string

(number) @number

(nil) @constant.builtin

[
  (false)
  (true)
] @boolean

(vararg_expression) @variable.builtin

; Keywords
[
  "return"
  "local"
  "type"
  "export"
  "do"
  "end"
  "while"
  "repeat"
  "until"
  "if"
  "elseif"
  "else"
  "then"
  "for"
  "function"
  "in"
  "and"
  "not"
  "or"
  "typeof"
] @keyword

[
  (break_statement)
  (continue_statement)
] @keyword

; Operators
[
  "+"
  "-"
  "*"
  "/"
  "%"
  "^"
  "#"
  "=="
  "~="
  "<="
  ">="
  "<"
  ">"
  "="
  "&"
  "|"
  "?"
  "//"
  ".."
  "+="
  "-="
  "*="
  "/="
  "%="
  "^="
  "..="
  "//="
] @operator

; Punctuation
[
  ";"
  ":"
  "::"
  ","
  "."
  "->"
] @punctuation.delimiter

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

(variable_list
  attribute: (attribute
    (identifier) @attribute))
