-- `heiko.values`: tables as values. A list read from JSON carries the
-- host's mark (`array`), which says it is a list even when empty.

return function(heiko)
local values = {}
heiko.values = values

local ARRAY = getmetatable(array({}))

-- How many keys `map` has.
function values.count(map)
	local n = 0
	for _ in pairs(map) do
		n += 1
	end
	return n
end

-- A copy of `value`, however deep: changing one changes nothing of the
-- other. A list stays a list.
function values.copy(value)
	if type(value) ~= "table" then
		return value
	end
	local out = {}
	for key, item in pairs(value) do
		out[key] = values.copy(item)
	end
	local meta = getmetatable(value)
	if type(meta) == "table" then
		return setmetatable(out, meta)
	elseif meta ~= nil then
		-- The host's mark is locked: a list is marked again.
		return array(out)
	end
	return out
end

-- Whether `a` and `b` hold the same, however deep.
function values.same(a, b)
	if type(a) ~= "table" or type(b) ~= "table" then
		return a == b
	end
	for key, value in pairs(a) do
		if not values.same(value, b[key]) then
			return false
		end
	end
	for key in pairs(b) do
		if a[key] == nil then
			return false
		end
	end
	return true
end

-- Whether `value` is a list: marked as one, or a table that is not
-- empty and has only numbers for keys.
function values.is_array(value)
	if type(value) ~= "table" then
		return false
	end
	if ARRAY ~= nil and getmetatable(value) == ARRAY then
		return true
	end
	if next(value) == nil then
		return false
	end
	for key in pairs(value) do
		if type(key) ~= "number" then
			return false
		end
	end
	return true
end

-- Whether `list` holds `value`.
function values.contains(list, value)
	for _, item in ipairs(list or {}) do
		if item == value then
			return true
		end
	end
	return false
end

end
