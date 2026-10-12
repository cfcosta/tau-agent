# tau-rlm-eval

A real-model evaluation of how tau copes with an input too large to read,
after "Recursive Language Models" (Zhang, Kraska and Khattab, arXiv
2512.24601). An RLM keeps the long input outside the model's context,
works on it from code, and calls the model recursively on slices. tau has
the parts (`codemode`, `tools.infer`, `parallel`/`map`, artifacts); this
measures whether they get used and whether they pay. It makes real model
calls and costs money.

## Tasks

One generated file, `data/tickets.txt`, of customer support messages
(about 100 bytes a line), the same for every arm of a trial. Each message
is mainly about one of five things; some deliberately share words with
another (a message asking for a refund *button* is a feature request).

| Task     | Question                                    | Needs                         |
| -------- | ------------------------------------------- | ----------------------------- |
| `count`  | how many messages are billing messages      | every line classified (OOLONG) |
| `top`    | which category has the most messages        | every line, five counts       |
| `needle` | the passphrase hidden in one line           | a `grep`: the control         |

The answer is the last `ANSWER:` line of the reply. `exact` is a match;
`score` is 1 for a match and, for `count`, 1 minus the relative error
otherwise. Inputs come from a seed, so a size and trial are the same on
every run.

## Arms

| Arm             | What the model has                                                          |
| --------------- | --------------------------------------------------------------------------- |
| `plain`         | the coding tools                                                            |
| `codemode`      | plus `codemode`, with `tools.infer` at its default limits (16 calls, 100k tokens, US$1, 60 s) |
| `codemode_wide` | the same with the limits raised (400 calls, 8 at once, 20 min, US$10)       |
| `rlm`           | `codemode_wide` and the paper's strategy, with a worked Luau example, in the instructions |

## Usage

```sh
cargo run --release -p tau-rlm-eval -- --task count --lines 1000,4000 --budget-usd 15 --json out.json
```

Access is tau's saved ChatGPT sign-in (`--chatgpt ID` to pick the account).
Options: `--task`, `--arm`, `--lines`, `--trials`, `--model`, `--budget-usd`,
`--json`, `--max-turns`, `--work`.

## Reading the results

The cost of a run includes the `tools.infer` calls a script made. `infer`
counts those calls. A coding agent can sometimes answer `count` with a
script of its own (regular expressions over the file); `exact` below 1 for
`plain` is that script missing the messages that share words across
categories. Results of runs are recorded in
`docs/reference/rlm-evaluation.md`.
