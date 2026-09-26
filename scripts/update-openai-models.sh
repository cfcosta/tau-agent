#!/usr/bin/env bash
# Refreshes crates/tau-ai/data/models-dev-openai.json from models.dev.
#
# Ported from pi's `packages/ai/scripts/generate-models.ts` (checkout at
# 2b0a123): that script fetches https://models.dev/api.json, walks
# `data.openai.models`, and keeps every model regardless of `tool_call`
# (the `tool_call === true` filter and the
# `MODELS_DEV_OPENAI_UNSUPPORTED_MODEL_IDS` exclusion are applied later,
# in Rust, by `tau_ai::model`) so that a refresh can never silently drop
# a model tau-ai's build-time filter should have seen.
#
# Usage: bash scripts/update-openai-models.sh
set -euo pipefail

script_path="$(realpath -- "${BASH_SOURCE[0]}")"
root_dir="$(dirname -- "$(dirname -- "$script_path")")"
out_file="$root_dir/crates/tau-ai/data/models-dev-openai.json"
source_url="https://models.dev/api.json"
fetched_date="$(date -u +%Y-%m-%d)"

tmp_file="$(mktemp)"
trap 'rm -f "$tmp_file"' EXIT

echo "Fetching $source_url ..." >&2
curl --fail --silent --show-error --location "$source_url" -o "$tmp_file"

# Reduce every OpenAI model to the fields tau_ai::model reads (mirrors the
# fields generate-models.ts reads off `ModelsDevModel`: `tool_call`,
# `reasoning`, `modalities.input`, `cost.{input,output,cache_read,
# cache_write}`, `limit.{context,output}`), and sort by id so the vendored
# file diffs cleanly on every refresh.
jq --arg source "$source_url" --arg fetched "$fetched_date" '
  {
    source: $source,
    fetched: $fetched,
    models: (
      .openai.models
      | to_entries
      | sort_by(.key)
      | map({
          key: .key,
          value: {
            id: .key,
            name: (.value.name // .key),
            reasoning: (.value.reasoning == true),
            tool_call: (.value.tool_call == true),
            modalities: { input: (.value.modalities.input // []) },
            cost: {
              input: (.value.cost.input // 0),
              output: (.value.cost.output // 0),
              cache_read: (.value.cost.cache_read // 0),
              cache_write: (.value.cost.cache_write // 0)
            },
            limit: {
              context: (.value.limit.context // 0),
              output: (.value.limit.output // 0)
            }
          }
        })
      | from_entries
    )
  }
' "$tmp_file" > "$out_file"

total=$(jq '.models | length' "$out_file")
tool_call=$(jq '[.models[] | select(.tool_call == true)] | length' "$out_file")
reasoning=$(jq '[.models[] | select(.reasoning == true)] | length' "$out_file")

echo "Wrote $out_file"
echo "  fetched:              $fetched_date"
echo "  models total:         $total"
echo "  models with tool_call: $tool_call"
echo "  models with reasoning: $reasoning"
