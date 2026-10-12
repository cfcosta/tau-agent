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
- One trial per cell is noise: the 1000-line `plain` miss was 200 for
  213.

## Not yet shown

An input that does not compress: real text where a message's class is
not a few phrases, and tasks over pairs of lines (the paper's
OOLONG-Pairs), where a script cannot cluster its way out. Until
there is one, this does not say whether the RLM parts are worth their
cost, only that for classification of templated data they are not.
