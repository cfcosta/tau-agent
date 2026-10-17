# Relabel script

Write this to a scratch file outside the repository (for example `/tmp/<slug>/relabel.sh`) and run it with `exec_command`. Its output says only how many candidates it relabelled. Do not `read` or `cat` the mapping file until you have scored the candidates.

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

The scrub file holds one word per line: the variant file names, the skill names that differ between variants, the candidate paths, and any phrase only one variant's text contains. Check one relabelled folder with `ls` (not its contents) to confirm the copy worked.

What it does not hide: the judge forked your conversation and saw the spawn tasks and landing reports. Content that reveals the variant in ways the scrub list misses (style, structure) also stays. Say this in the reply.
