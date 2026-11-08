-- A constitution (docs/reference/constitution.md): rules, where each
-- applies, and how sure Jev must be that one is broken to review or
-- block, with `on_error` and `max_holds`. Each repository's lives in
-- the plugin's store, and is edited only through tau's UI. Every rule
-- is checked when it is added, replaced or read back.
--
-- A rule applies to tool arguments (`edit.newText` is every `newText`
-- in an `edit` call's arguments, however deep) or to the run's final
-- answer (`final answer`).
local rules = {}

rules.DEFAULT_MAX_HOLDS = 3
rules.DEFAULT_REVIEW = 0.5
rules.DEFAULT_BLOCK = 0.8

-- What a target that is the final answer reads back as.
rules.FINAL_ANSWER = "final answer"

-- White space as Unicode has it, by code point: what Rust's `trim`
-- takes off.
local SPACE = {}
for _, c in ipairs({ 0x20, 0x85, 0xA0, 0x1680, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000 }) do
	SPACE[c] = true
end
for c = 0x09, 0x0D do
	SPACE[c] = true
end
for c = 0x2000, 0x200A do
	SPACE[c] = true
end

-- `text` without the white space it starts and ends with.
function rules.trim(text)
	local first, last
	for at, c in utf8.codes(text) do
		if not SPACE[c] then
			first = first or at
			last = at
		end
	end
	if first == nil then
		return ""
	end
	-- `last` starts the last character kept: the cut is after it.
	local after = utf8.offset(text, 2, last) or (#text + 1)
	return string.sub(text, first, after - 1)
end

-- A number as Rust's `{}` writes it: the fewest digits that read back,
-- never with an exponent.
function rules.float(x)
	if x ~= x then
		return "NaN"
	elseif x == math.huge then
		return "inf"
	elseif x == -math.huge then
		return "-inf"
	elseif x == 0 then
		return 1 / x < 0 and "-0" or "0"
	end
	local shortest
	for digits = 1, 17 do
		shortest = string.format("%." .. digits .. "g", x)
		if tonumber(shortest) == x then
			break
		end
	end
	local sign = ""
	if string.sub(shortest, 1, 1) == "-" then
		sign, shortest = "-", string.sub(shortest, 2)
	end
	local mantissa, exponent = string.match(shortest, "^([%d%.]+)e([%+%-]%d+)$")
	if mantissa == nil then
		return sign .. shortest
	end
	local whole, fraction = string.match(mantissa, "^(%d+)%.?(%d*)$")
	local all = whole .. fraction
	local point = #whole + tonumber(exponent)
	if point <= 0 then
		return sign .. "0." .. string.rep("0", -point) .. all
	elseif point >= #all then
		return sign .. all .. string.rep("0", point - #all)
	end
	return sign .. string.sub(all, 1, point) .. "." .. string.sub(all, point + 1)
end

-- Why a rule, or part of one, does not check out, worded for the UI.
local errors = {}
rules.errors = errors

function errors.no_target(text)
	return "`" .. text .. "` names no target: write `tool.field` (such as `edit.newText`) or `final answer`"
end

function errors.not_on_error(text)
	return "`" .. text .. "` is not allow or block"
end

errors.NO_ID = "A rule has no id"

function errors.no_text(id)
	return "Rule " .. id .. " has no text"
end

function errors.nowhere(id)
	return "Rule " .. id .. " applies nowhere: give it somewhere to apply"
end

function errors.target(id, why)
	return "Rule " .. id .. ": " .. why
end

function errors.not_probabilities(id)
	return "Rule " .. id .. ": review and block are probabilities, 0 to 1"
end

function errors.review_above_block(id, review, block)
	return "Rule " .. id .. ": review (" .. rules.float(review) .. ") is above block (" .. rules.float(block) .. ")"
end

function errors.duplicate(id)
	return "Two rules are called " .. id
end

function errors.unknown(id)
	return "There is no rule " .. id
end

-- Rules stored for `repo` that do not check out, and why.
function errors.invalid(repo, why)
	return "The constitution stored for " .. repo .. " is not valid: " .. why
end

-- Where `text` says a rule applies: `{ final = true }`, or `{ tool,
-- field }`; else nil and why. `edit.newText` is every `newText` in an
-- `edit` call's arguments.
function rules.target(text)
	text = rules.trim(text)
	-- Any case, as ASCII has it.
	if string.lower(text) == rules.FINAL_ANSWER then
		return { final = true }
	end
	local dot = string.find(text, ".", 1, true)
	if dot ~= nil and dot > 1 and dot < #text then
		return { tool = string.sub(text, 1, dot - 1), field = string.sub(text, dot + 1) }
	end
	return nil, errors.no_target(text)
end

-- How a target reads back: `tool.field`, or `final answer`.
function rules.label(target)
	if target.final then
		return rules.FINAL_ANSWER
	end
	return target.tool .. "." .. target.field
end

local function probability(p)
	return type(p) == "number" and p >= 0 and p <= 1
end

-- A rule, checked: an id, text, somewhere to apply, and thresholds that
-- are probabilities with review at most block. `{ id, text, on, review,
-- block }`, `on` its targets as labels; else nil and why.
function rules.checked(id, text, on, review, block)
	id = rules.trim(id)
	if id == "" then
		return nil, errors.NO_ID
	end
	if rules.trim(text) == "" then
		return nil, errors.no_text(id)
	end
	if #on == 0 then
		return nil, errors.nowhere(id)
	end
	local labels = array({})
	for _, place in ipairs(on) do
		local target, why = rules.target(place)
		if target == nil then
			return nil, errors.target(id, why)
		end
		table.insert(labels, rules.label(target))
	end
	if not probability(review) or not probability(block) then
		return nil, errors.not_probabilities(id)
	end
	if review > block then
		return nil, errors.review_above_block(id, review, block)
	end
	return { id = id, text = rules.trim(text), on = labels, review = review, block = block }
end

-- An empty constitution: no rules, Jev's failures let through, three
-- holds.
function rules.default()
	return { rules = array({}), on_error = "allow", max_holds = rules.DEFAULT_MAX_HOLDS }
end

-- A constitution read back from the store (`{ on_error, max_holds,
-- rules = { { id, text, on, review, block } } }`), every rule checked;
-- else nil and why. None stored is no rules.
function rules.from_stored(stored)
	if stored == nil then
		return rules.default()
	end
	local checked = array({})
	local known = {}
	for _, rule in ipairs(stored.rules or {}) do
		local ok, why = rules.checked(rule.id or "", rule.text or "", rule.on or {}, rule.review, rule.block)
		if ok == nil then
			return nil, why
		end
		if known[ok.id] then
			return nil, errors.duplicate(ok.id)
		end
		known[ok.id] = true
		table.insert(checked, ok)
	end
	local on_error = stored.on_error
	if on_error ~= "allow" and on_error ~= "block" then
		return nil, errors.not_on_error(tostring(on_error))
	end
	return { rules = checked, on_error = on_error, max_holds = stored.max_holds or rules.DEFAULT_MAX_HOLDS }
end

local function position(constitution, id)
	for at, rule in ipairs(constitution.rules) do
		if rule.id == id then
			return at
		end
	end
	return nil
end

-- Adds a rule, checked, under the lowest free id (`R1`, `R2`…): its id;
-- else nil and why, and nothing changes.
function rules.add(constitution, text, on, review, block)
	local n = 1
	while position(constitution, "R" .. n) ~= nil do
		n += 1
	end
	local rule, why = rules.checked("R" .. n, text, on, review, block)
	if rule == nil then
		return nil, why
	end
	table.insert(constitution.rules, rule)
	return rule.id
end

-- Rewrites the rule `id` in place: same id, same position, checked;
-- else nil and why, and nothing changes.
function rules.replace(constitution, id, text, on, review, block)
	local at = position(constitution, id)
	if at == nil then
		return nil, errors.unknown(id)
	end
	local rule, why = rules.checked(id, text, on, review, block)
	if rule == nil then
		return nil, why
	end
	constitution.rules[at] = rule
	return true
end

-- Removes the rule `id`: whether there was one.
function rules.remove(constitution, id)
	local at = position(constitution, id)
	if at == nil then
		return false
	end
	table.remove(constitution.rules, at)
	return true
end

-- The rule `id`, or nil.
function rules.find(constitution, id)
	local at = position(constitution, id)
	return at and constitution.rules[at]
end

-- Whether `value` is a JSON array, as Luau holds one.
local ARRAY = getmetatable(array({}))

local function is_array(value)
	return getmetatable(value) == ARRAY or value[1] ~= nil
end

-- Every value under `key` in `value`, however deep, into `found`. A
-- table keeps no order, so an object's keys are taken in sorted order.
local function collect(value, key, found)
	if type(value) ~= "table" then
		return
	end
	if is_array(value) then
		for _, item in ipairs(value) do
			collect(item, key, found)
		end
		return
	end
	local names = {}
	for name in pairs(value) do
		table.insert(names, name)
	end
	table.sort(names)
	for _, name in ipairs(names) do
		if name == key then
			table.insert(found, value[name])
		else
			collect(value[name], key, found)
		end
	end
end
rules.collect = collect

-- The rules for a call to `tool` with `args`, each with what it is
-- about: `{ rule, fields, names }`, `fields` the values of its fields
-- by name (one value, or a list of each found) and `names` their names
-- in the order the rule gives them. A rule none of whose fields occur
-- is not about the call.
function rules.for_call(constitution, tool, args)
	local about = {}
	for _, rule in ipairs(constitution.rules) do
		local fields, names = {}, {}
		for _, place in ipairs(rule.on) do
			local target = rules.target(place)
			if target and not target.final and target.tool == tool then
				local found = {}
				collect(args, target.field, found)
				if #found > 0 then
					if fields[target.field] == nil then
						table.insert(names, target.field)
					end
					if #found == 1 then
						fields[target.field] = found[1]
					else
						fields[target.field] = array(found)
					end
				end
			end
		end
		if #names > 0 then
			table.insert(about, { rule = rule, fields = fields, names = names })
		end
	end
	return about
end

-- The rules for the final answer.
function rules.for_final_answer(constitution)
	local found = {}
	for _, rule in ipairs(constitution.rules) do
		if table.find(rule.on, rules.FINAL_ANSWER) then
			table.insert(found, rule)
		end
	end
	return found
end

-- The tools the rules read a field of.
function rules.tools(constitution)
	local tools = {}
	for _, rule in ipairs(constitution.rules) do
		for _, place in ipairs(rule.on) do
			local target = rules.target(place)
			if target and not target.final then
				tools[target.tool] = true
			end
		end
	end
	return tools
end

return rules
