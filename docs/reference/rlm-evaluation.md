# RLM evaluation (`tau-rlm-eval`)

How tau copes with an input too large to read, measured against a real
model, after "Recursive Language Models" (arXiv 2512.24601). The
harness, tasks and arms are in `crates/evals/tau-rlm-eval/README.md`.

- Status: harness built; first results below. 2026-10-11, `gpt-5.5`
  through the ChatGPT plan, one trial per cell.

## First results: `count`, templated support messages

| Lines | Arm             | Exact | Score | Turns | `infer` | Cost (US$) | Time |
| ----: | --------------- | ----: | ----: | ----: | ------: | ---------: | ---: |
|  1000 | `plain`         |     0 |  0.94 |     9 |       0 |       0.31 |  97s |
|  1000 | `codemode`      |     0 |  0.94 |    12 |       7 |       0.66 | 156s |
|  1000 | `codemode_wide` |     1 |  1.00 |     8 |       1 |       0.45 | 132s |
|  1000 | `rlm`           |     0 |  0.95 |    23 |      26 |       2.27 | 370s |
|  4000 | `plain`         |     1 |  1.00 |    10 |       0 |       0.38 | 102s |
|  4000 | `codemode`      |     1 |  1.00 |    10 |       1 |       0.45 | 122s |
|  4000 | `codemode_wide` |     1 |  1.00 |    12 |       6 |       0.67 | 278s |
|  4000 | `rlm`           |     1 |  1.00 |    15 |      68 |       5.42 | 536s |

(An earlier 1000-line run, on an input with only 60 distinct messages,
had `plain` exact for US$0.12 and `rlm` exact for US$1.13.)

## Real text: `docs`, tau's own documents

96 Markdown files (about 300k tokens), shuffled into one file under
`### DOCUMENT k` lines, titles removed; the question is how many are
about codemode (27, by path). Two trials per arm; 27 is exact.

| Arm             | Answers          | Mean score | `infer` | Cost (US$) | Time |
| --------------- | ---------------- | ---------: | ------: | ---------: | ---: |
| `plain`         | 28, 28           |       0.96 |       0 |       0.62 | 115s |
| `codemode`      | 30, 27           |       0.94 |       1 |       0.57 | 122s |
| `codemode_wide` | 27, 27           |       1.00 |       0 |       0.43 |  71s |
| `rlm`           | 21, 25           |       0.85 |     118 |       2.71 | 182s |

The `rlm` arm is the worst and about five times dearer: a sub-model sees
one slice at a time and judges "about codemode" without the rest of the
project, where the main model, holding the file by its own scripts, judges
each document against the whole. `codemode_wide` made no `infer` calls, so
its edge over `codemode` is noise.

## GPT-6.1 Sol

The same `docs` (two trials) and `count` at 4000 lines (one trial) on
`gpt-6.1-sol`, about US$3.40 in all.

| Task    | Arm             | Exact | Score | `infer` | Cost (US$) | Time |
| ------- | --------------- | ----: | ----: | ------: | ---------: | ---: |
| `docs`  | `plain`         |  2/2  |  1.00 |       0 |       0.07 |  37s |
| `docs`  | `codemode`      |  2/2  |  1.00 |       0 |       0.09 |  34s |
| `docs`  | `codemode_wide` |  2/2  |  1.00 |       0 |       0.09 |  44s |
| `docs`  | `rlm`           |  0/2  |  0.89 |     187 |       1.01 | 251s |
| `count` | `plain`         |  1/1  |  1.00 |       0 |       0.03 |  34s |
| `count` | `codemode`      |  1/1  |  1.00 |       0 |       0.04 |  28s |
| `count` | `codemode_wide` |  1/1  |  1.00 |       0 |       0.06 |  40s |
| `count` | `rlm`           |  0/1  |  1.00 |      40 |       0.78 | 144s |

The stronger model gets every question right with the coding tools or
plain codemode, in half a minute for a few cents, and never reaches for
`infer` unless told to. The `rlm` arm, told to, is 15 to 25 times dearer
and misses (26 and 22 of 27; 889 of 892). The gap in cost against the
other arms grew with the model's strength.

## What it shows

- **Nothing here needed recursion.** The model with only the coding
  tools answers by cutting the file down with `sort`, `cut` and a
  script of its own, and gets within a few percent or exact. This input
  is generated from templates, so it compresses: a coding agent can
  cluster the messages and classify the clusters. The paper's inputs do
  not compress that way.
- **The paper's strategy costs 5 to 14 times more** here, for no gain:
  it classifies every chunk with the model, 68 `infer` calls and
  435k input tokens at 4000 lines.
- **The default `infer` limits did not bind** (7 calls at most, under
  the 16).
- A stronger model (above) widened the gap rather than closing it.
- One trial per cell is noise: the 1000-line `plain` miss was 200 for
  213.

## Decision

The parts of the paper not built (recursion below `infer`, script
variables kept across calls, the strategy as standing instructions) are
not built: on both inputs the arm that used them was dearer and no better,
and the one with the strategy in its instructions was worse on real text.
The worked example stays out of the codemode description for that reason.
Revisit with a task that scripts cannot reduce (pairs of lines, the
paper's OOLONG-Pairs, or a corpus where a document's class depends on its
whole text), and with more than two trials.

## Not yet shown

Tasks over pairs of lines (the paper's OOLONG-Pairs), where a script
cannot cluster its way out; `top` and `needle` (built, not run); more than
one or two trials per cell; other models.
