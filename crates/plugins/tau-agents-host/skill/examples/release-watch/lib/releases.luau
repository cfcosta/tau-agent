local releases = {}

-- A release is the same release whatever else changes about it.
function releases.key(release)
  return release.tag_name
end

return releases
