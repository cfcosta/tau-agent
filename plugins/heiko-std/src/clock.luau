-- `heiko.now`: the time a hook runs at.

return function(heiko)

-- The time of the step, in Unix seconds, from the `env` a hook is
-- given: the host's clock, which a test sets.
function heiko.now(env)
	return env.now.unix
end

end
