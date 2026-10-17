# Third-party notices

tau is under the MIT license ([LICENSE](LICENSE)). It includes work from
the projects below, under their own licenses. Rust dependencies come
with their own license files and are not listed here.

## pi

<https://github.com/earendil-works/pi>. Parts of tau are ported from
pi, and keep its behaviour, limits and some of its text:

- `tau-ai`: its message types and events, the Responses stream, retries
  and cost.
- `tau-agent`: the agent loop, its events, argument validation and the
  strict schema rewrite.
- `tau-compaction`: the summary compaction, its prompts and format.
- `tau-tools-host`: `read`, `write`, `edit`, `ls`, `grep` and `find`,
  with their truncation, paths, file locks and image handling.
- `tau-codemode` and `tau-codemode-host`: pi's Codemode
  (`packages/codemode`), rewritten for Luau.
- `tau-testing`: the scripted model.

MIT License

Copyright (c) 2025 Mario Zechner


Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## pstack

<https://github.com/cursor/plugins/tree/main/pstack>. The skills
`tau-skills` ships (`crates/plugins/tau-skills/skills`) are adapted
from pstack's: its principles, the poteto-mode playbooks, and `how`,
`why`, `architect`, `arena`, `interrogate`, `blast-radius`, `tdd`,
`technical-writing`, `correct`, `reflect`, `figure-it-out`, `teach` and
the verification skills, rewritten for tau's tools.

MIT License

Copyright (c) 2026 Lauren Tan

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## humanizer

<https://github.com/blader/humanizer>. `tau-skills` ships an adaptation
of its skill (version 2.4.1) as `humanizer`.

MIT License

Copyright (c) 2025 Siqi Chen

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Fast compaction

`tau-fast-compaction` ports the decision logic of three MIT-licensed
projects. Their notices are in
[crates/plugins/tau-fast-compaction/THIRD_PARTY_NOTICES.md](crates/plugins/tau-fast-compaction/THIRD_PARTY_NOTICES.md).

## Fonts

`tau-ui-kit` builds these fonts into the app. They are under the SIL
Open Font License 1.1, whose text must go with any copy of tau that
includes them:

- Geist and Geist Mono, Copyright (c) 2023 Vercel, in collaboration
  with basement.studio:
  [crates/tau-ui-kit/assets/fonts/OFL.txt](crates/tau-ui-kit/assets/fonts/OFL.txt).
- Newsreader, Copyright 2020 The Newsreader Project Authors:
  [crates/tau-ui-kit/assets/fonts/OFL-Newsreader.txt](crates/tau-ui-kit/assets/fonts/OFL-Newsreader.txt).

## Designs followed, not copied

These shaped how parts of tau work. No code or text of theirs is in tau.

- OptChat by Victor Taelin
  (<https://gist.github.com/VictorTaelin/91837951a5ce5b38f341ec1ba1df6449>):
  the tree of summaries in `tau-tree-compaction`, and the rules the
  compaction and memory prompts follow. Those prompts are tau's own
  words.
- Codex's unified exec (<https://github.com/openai/codex>): how
  `exec_command` and `write_stdin` behave.
