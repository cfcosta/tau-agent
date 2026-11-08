-- Times as tau-todo keeps them, seconds since 1970 in UTC, and as beads
-- writes them, `2026-11-05T14:00:00Z`.

local time = {}

-- Days since 1970-01-01 of a civil date.
local function days_from_civil(y, m, d)
	if m <= 2 then
		y -= 1
	end
	local era = (y >= 0 and y or y - 399) // 400
	local yoe = y - era * 400
	local mp = (m + 9) % 12
	local doy = (153 * mp + 2) // 5 + d - 1
	local doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
	return era * 146097 + doe - 719468
end

-- The civil date of days since 1970-01-01.
local function civil_from_days(z)
	z += 719468
	local era = (z >= 0 and z or z - 146096) // 146097
	local doe = z - era * 146097
	local yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
	local y = yoe + era * 400
	local doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
	local mp = (5 * doy + 2) // 153
	local d = doy - (153 * mp + 2) // 5 + 1
	local m = mp < 10 and mp + 3 or mp - 9
	if m <= 2 then
		y += 1
	end
	return y, m, d
end

-- `unix` as `YYYY-MM-DDTHH:MM:SSZ`.
function time.iso(unix)
	unix = math.floor(unix)
	local days = unix // 86400
	local rest = unix - days * 86400
	local y, m, d = civil_from_days(days)
	return string.format(
		"%04d-%02d-%02dT%02d:%02d:%02dZ",
		y,
		m,
		d,
		rest // 3600,
		(rest % 3600) // 60,
		rest % 60
	)
end

-- `text`, a date (`2026-11-10`) or a time (`2026-11-10T09:00:00Z`, any
-- fraction of a second dropped), as seconds since 1970; nil if it is
-- neither.
function time.parse(text)
	if type(text) == "number" then
		return math.floor(text)
	end
	if type(text) ~= "string" then
		return nil
	end
	local y, m, d = string.match(text, "^(%d%d%d%d)%-(%d%d)%-(%d%d)")
	if not y then
		return nil
	end
	y, m, d = tonumber(y), tonumber(m), tonumber(d)
	if m < 1 or m > 12 or d < 1 or d > 31 then
		return nil
	end
	local unix = days_from_civil(y, m, d) * 86400
	local hh, mm, ss = string.match(text, "^%d%d%d%d%-%d%d%-%d%d[T ](%d%d):(%d%d):(%d%d)")
	if hh then
		unix += tonumber(hh) * 3600 + tonumber(mm) * 60 + tonumber(ss)
	end
	return unix
end

return time
