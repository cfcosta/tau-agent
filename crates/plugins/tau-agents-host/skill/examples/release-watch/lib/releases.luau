local releases = {}

-- A release is the same release whatever else changes about it.
function releases.key(release)
  return release.tag_name
end

-- The releases a result holds that are out: not drafts.
function releases.published(result)
  local out = {}
  for _, release in ipairs(result) do
    if not release.draft then
      table.insert(out, release)
    end
  end
  return out
end

return releases
