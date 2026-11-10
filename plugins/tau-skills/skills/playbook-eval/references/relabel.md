# Relabelling candidates for the judge

The judge is fresh: it knows only what its task says. Relabelling makes sure the task names no variant. Pick one of two ways.

## Labelled change ids (the default)

Use this when the rubric needs commits (their order, their messages) or the outputs do not quote the variant text.

Write one line per candidate to a scratch file outside the repository, for example `/tmp/<slug>/candidates.tsv`: the variant, a tab, then the candidate's change ids oldest first, separated by spaces, as tau's report turn named them. Then run this with `exec_command`:

```bash
#!/usr/bin/env bash
# usage: relabel-ids.sh <out-dir> <candidates.tsv>
set -euo pipefail
out=$1; list=$2
: > "$out/mapping.tsv"; : > "$out/judge.txt"
labels=(A B C D E F G H)
i=0
while IFS=$'\t' read -r variant changes; do
  label=${labels[$i]}; i=$((i + 1))
  printf '%s\t%s\t%s\n' "$label" "$variant" "$changes" >> "$out/mapping.tsv"
  printf 'Candidate %s: changes %s (oldest first; the last holds all its files)\n' "$label" "$changes" >> "$out/judge.txt"
done < <(shuf "$list")
echo "relabelled $i candidates into $out/judge.txt"
```

Paste `judge.txt` into the judge's task. Tell the judge to read each candidate with `vcs_show {change}`. Do not read `mapping.tsv` until you have scored the candidates yourself and the judge has reported.

Check the candidates' commit messages with `vcs_log` before you spawn the judge. If one names its variant or quotes the variant's text, use the second way.

## Scrubbed copies

Use this when the outputs might name the variant. For each candidate, take its files into your working copy with `vcs_restore {paths: [<path>], from: <its last change>}`, copy the path to a scratch folder outside the repository (for example `/tmp/<slug>/raw/<n>/`), then put your own files back with `vcs_restore {paths: [<path>]}`. Do not commit any of it. Then run:

```bash
#!/usr/bin/env bash
# usage: relabel.sh <out-dir> <scrub-words-file> <candidate-path>...
set -euo pipefail
out=$1; scrub=$2; shift 2
rm -rf "$out/blind"; mkdir -p "$out/blind"
: > "$out/mapping.tsv"
labels=(A B C D E F G H)
i=0
while IFS= read -r src; do
  label=${labels[$i]}; i=$((i + 1))
  cp -r "$src" "$out/blind/$label"
  printf '%s\t%s\n' "$label" "$src" >> "$out/mapping.tsv"
done < <(printf '%s\n' "$@" | shuf)
while IFS= read -r word; do
  [ -n "$word" ] || continue
  grep -rlF -- "$word" "$out/blind" | while IFS= read -r f; do
    sed -i "s|$word|[variant]|g" "$f"
  done || true
done < "$scrub"
echo "relabelled $i candidates into $out/blind"
```

The scrub file holds one word per line: the variant file names, the skill names that differ between variants, and any phrase only one variant's text contains. Check one relabelled folder with `ls` (not its contents) to confirm the copy worked. Give the judge the path of `blind/`. The copies carry no commits, so a rubric criterion about commit order cannot be judged this way.

## What it does not hide

Content that reveals the variant in ways the labels and scrub list miss (style, structure) stays. Say this in the reply.
