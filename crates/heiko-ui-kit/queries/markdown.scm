; Markdown's blocks, in the names heiko-ui-kit colors: a heading as a
; function's name, its markers and a list's as operators, code as a
; string, front matter as a comment. The grammar's own query is
; Neovim's, whose names (`text.title`) nothing here reads.

(atx_heading
  (inline) @function)

(setext_heading
  (paragraph) @function)

[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
  (thematic_break)
  (block_quote_marker)
] @punctuation.special

[
  (indented_code_block)
  (fenced_code_block)
] @string

(fenced_code_block_delimiter) @punctuation

(info_string) @type

(link_destination) @string.special

(link_label) @label

(minus_metadata) @comment
