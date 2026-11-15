-- `heiko.count`: numbers as the interface says them.

return function(heiko)
local count = {}
heiko.count = count

-- `n` of a thing: `1 note`, `3 notes`. `many` is `one` with an `s`
-- unless given.
function count.plural(n, one, many)
	if n == 1 then
		return "1 " .. one
	end
	return tostring(n) .. " " .. (many or (one .. "s"))
end

-- A count of tokens, short: `900`, `184k`, `1.2M`. Thousands are
-- rounded down, so a count never reads as more than it is.
function count.tokens(n)
	n = math.floor(n)
	if n < 1000 then
		return tostring(n)
	elseif n < 1000000 then
		return (n // 1000) .. "k"
	end
	return string.format("%.1fM", (n // 100000) / 10)
end

end
