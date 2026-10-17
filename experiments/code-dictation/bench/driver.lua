-- Runs one benchmark job inside `nvim --headless -n -u NONE -i NONE`:
-- loads the real ears_code.lua, then plays each utterance through it as the
-- ears daemon would (a VoiceCommand when the whole utterance is an ears
-- command, else SpeechStarted + SegmentCompleted), and records the buffer,
-- cursor and mode after every step. Keys the plugin feeds (mode switches,
-- Vim commands) are processed by Neovim's own input loop between steps.
--
-- Job: JSON at $EARS_BENCH_JOB; result: JSON at job.out.
local job = vim.json.decode(table.concat(vim.fn.readfile(os.getenv("EARS_BENCH_JOB")), "\n"))

local co
local function resume()
  local ok, err = coroutine.resume(co)
  if not ok then
    vim.fn.writefile({ vim.json.encode({ fatal = debug.traceback(co, tostring(err)) }) }, job.out)
    vim.cmd("qa!")
  end
end
local function sleep(ms)
  vim.defer_fn(resume, ms)
  coroutine.yield()
end

local function now() return vim.uv.hrtime() / 1e6 end

local function keys(k)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(k, true, false, true), "m", false)
end

local function mode()
  local m = vim.api.nvim_get_mode().mode
  if m:match("^[iR]") then return "i" end
  if m:match("^[vV\22]") then return "v" end
  return "n"
end

-- What the plugin said: warnings ("not found", "not a command") and the
-- keys it fed ("ears: heard → keys").
local messages = {}
vim.notify = function(msg) table.insert(messages, tostring(msg)) end
vim.api.nvim_echo = function(chunks)
  local text = {}
  for _, c in ipairs(chunks) do table.insert(text, c[1]) end
  table.insert(messages, table.concat(text))
end

-- ears' own command matching (src/commands.rs): the whole utterance,
-- lowercased, punctuation as spaces.
local function normalize(text)
  return vim.trim((text:lower():gsub("[^%w]", " "):gsub("%s+", " ")))
end
local COUNTS = { one = 1, won = 1, two = 2, to = 2, too = 2, three = 3, four = 4, ["for"] = 4,
  five = 5, six = 6, seven = 7, eight = 8, nine = 9, ten = 10 }
local function ears_command(text)
  local c = job.commands
  if not c or not c.enabled then return nil end
  local said = normalize(text)
  if said == "" then return nil end
  local function is(list)
    for _, p in ipairs(list or {}) do
      if normalize(p) == said then return true end
    end
  end
  if is(c.enter) then return "enter" end
  if is(c.new_line) then return "new_line" end
  if is(c.undo) then return "undo" end
  if is(c.backtrack) then return "backtrack" end
  for _, word in ipairs(c.back or {}) do
    local rest = said:match("^" .. vim.pesc(normalize(word)) .. " (%S+)$")
    local n = rest and (COUNTS[rest] or tonumber(rest))
    if n and n >= 1 and n <= 10 then return "back:" .. n end
  end
  for phrase, combo in pairs(c.keys or {}) do
    if normalize(phrase) == said then return "keys:" .. combo end
  end
  for _, word in ipairs(c.literal or {}) do
    local w = normalize(word)
    if said:sub(1, #w + 1) == w .. " " then return "literal" end
  end
end

-- The names the plugin sends as scope, computed the same way (scope_file in
-- ears_code.lua): the ASR context in audio mode.
local function scope()
  local counts = {}
  for _, line in ipairs(vim.api.nvim_buf_get_lines(0, 0, -1, false)) do
    for word in line:gmatch("[%a_][%w_]+") do counts[word] = (counts[word] or 0) + 1 end
  end
  local names = vim.tbl_keys(counts)
  table.sort(names, function(a, b) return counts[a] > counts[b] end)
  return table.concat(names, " ", 1, math.min(#names, 300))
end

local function snapshot()
  local cur = vim.api.nvim_win_get_cursor(0)
  return { lines = vim.api.nvim_buf_get_lines(0, 0, -1, false), row = cur[1], col = cur[2], mode = mode() }
end

-- Wait until the translator has replied and the fed keys have run.
local function settle(M, t0)
  local replied
  while now() - t0 < job.timeout_ms do
    if next(M.state.pending) == nil then
      replied = replied or now()
      break
    end
    sleep(2)
  end
  local last, stable = nil, 0
  while stable < 3 and now() - t0 < job.timeout_ms do
    sleep(10)
    local s = vim.json.encode(snapshot())
    stable = s == last and stable + 1 or 0
    last = s
  end
  return replied and (replied - t0) or nil
end

local function set_state(lines, row, col, m)
  if mode() ~= "n" then keys("<Esc>"); sleep(20) end
  vim.api.nvim_buf_set_lines(0, 0, -1, false, lines)
  row = math.min(row, #lines)
  local len = #lines[row]
  if m == "i" then
    if len > 0 and col >= len then
      vim.api.nvim_win_set_cursor(0, { row, len - 1 })
      keys("a")
    else
      vim.api.nvim_win_set_cursor(0, { row, col })
      keys("i")
    end
  else
    vim.api.nvim_win_set_cursor(0, { row, math.min(col, math.max(len - 1, 0)) })
  end
  sleep(20)
end

local function hear(step)
  if not step.wav then return step.text, nil end
  local out = vim.system({ "python3", job.hear, step.wav }, { stdin = scope(), text = true }):wait()
  if out.code ~= 0 then return "", nil, out.stderr end
  local r = vim.json.decode(out.stdout)
  return r.text, r.ms, nil, r.cached
end

co = coroutine.create(function()
  vim.cmd("filetype plugin indent on")
  vim.api.nvim_buf_set_name(0, job.buf_name)
  vim.bo.filetype = job.filetype
  local M = dofile(job.plugin)
  local s = job.start
  set_state(job.start_lines, s.row, s.col, s.mode)

  local results = {}
  for _, step in ipairs(job.steps) do
    -- Resync: start every step from the state the ideal run reached.
    if job.resync and step.before and step.before ~= vim.NIL then
      local b = step.before
      set_state(b.lines, b.row, b.col, b.mode)
    end
    messages = {}
    vim.v.errmsg = ""
    local heard, asr_ms, asr_err, cached
    if not job.golden then heard, asr_ms, asr_err, cached = hear(step) end
    local command = not job.golden and ears_command(heard)
    local t0 = now()
    local ok, err = pcall(function()
      if job.golden then
        -- The ideal effect of the utterance: keys, or lines as the plugin
        -- inserts them.
        if step["do"].keys then keys(step["do"].keys) else M.insert_lines(step["do"].lines) end
      elseif command then
        M.on_event({ VoiceCommand = { name = command, text = heard } })
      else
        M.on_event("SpeechStarted")
        M.on_event({ SegmentCompleted = { text = heard } })
      end
    end)
    local translate_ms = settle(M, t0)
    local snap = snapshot()
    snap.index, snap.heard, snap.event = step.index, heard, command and ("VoiceCommand:" .. command) or "SegmentCompleted"
    snap.asr_ms, snap.asr_cached, snap.translate_ms = asr_ms, cached, not command and translate_ms or nil
    snap.messages = messages
    snap.error = (not ok and tostring(err)) or asr_err or (vim.v.errmsg ~= "" and vim.v.errmsg) or nil
    table.insert(results, snap)
  end
  vim.fn.writefile({ vim.json.encode({ steps = results }) }, job.out)
  if M.state.translator then M.state.translator:kill("sigterm") end
  vim.cmd("qa!")
end)

vim.schedule(resume)
