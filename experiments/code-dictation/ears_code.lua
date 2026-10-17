-- Push-to-talk code dictation for Neovim (experiment).
-- <M-v> (Alt-v, normal or insert): start talking; press again to stop.
-- While you talk, the recording so far is re-transcribed every PARTIAL_MS and
-- the translated code shows as ghost text where it will land. On stop, the
-- final transcription is inserted as one line (one undo step). Beam-search
-- hypotheses follow in the background and, when they translate differently,
-- open as a completion menu over the inserted line.
local here = debug.getinfo(1, "S").source:sub(2):match("(.*/)")
local PARTIAL_MS = 500
local ns = vim.api.nvim_create_namespace("ears_code")
vim.api.nvim_set_hl(0, "EarsGhost", { fg = "#ffcc33", italic = true, default = true })
vim.api.nvim_set_hl(0, "EarsHeard", { link = "Comment", default = true })

local state = { rec = nil, wav = nil, timer = nil, busy = false, scope = nil, buf = nil, lang = nil }

local function device()
  local f = io.open(vim.fn.expand("~/.config/ears/config.toml"))
  if not f then return nil end
  local text = f:read("*a")
  f:close()
  return text:match('\ndevice%s*=%s*"([^"]+)"') or text:match('^device%s*=%s*"([^"]+)"')
end

local function lang()
  local ft = vim.bo.filetype
  if ft == "python" then return "py" end
  if ft == "sh" or ft == "bash" or ft == "zsh" then return "sh" end
  return "text"
end

-- Names in this buffer, most frequent first: the context that lets the model
-- hear `parse_args` instead of "pass args".
local function scope_file()
  local counts = {}
  for _, line in ipairs(vim.api.nvim_buf_get_lines(0, 0, -1, false)) do
    for word in line:gmatch("[%a_][%w_]+") do
      counts[word] = (counts[word] or 0) + 1
    end
  end
  local names = vim.tbl_keys(counts)
  table.sort(names, function(a, b) return counts[a] > counts[b] end)
  local path = vim.fn.tempname()
  vim.fn.writefile({ table.concat(names, " ", 1, math.min(#names, 300)) }, path)
  return path
end

local function indent_for(row, code)
  local prev = vim.api.nvim_buf_get_lines(0, row - 1, row, false)[1] or ""
  local indent = prev:match("^%s*")
  local sw = vim.fn.shiftwidth()
  if prev:match(":%s*$") and vim.bo.filetype == "python" then
    indent = indent .. string.rep(" ", sw)
  end
  if code:match("^el") or code:match("^except") or code:match("^finally") then
    indent = indent:sub(sw + 1)
  end
  return indent
end

-- Where a line goes: over a blank current line, otherwise below it.
local function target()
  local row = vim.api.nvim_win_get_cursor(0)[1]
  local blank = vim.api.nvim_get_current_line():match("^%s*$") ~= nil
  return row, blank
end

local function ghost(code, heard)
  vim.api.nvim_buf_clear_namespace(state.buf, ns, 0, -1)
  local row, blank = target()
  local chunks = { { code ~= "" and code or "…", "EarsGhost" } }
  if heard and heard ~= "" then
    table.insert(chunks, { "   ← " .. heard, "EarsHeard" })
  end
  if blank then
    table.insert(chunks, 1, { indent_for(row - 1, code) })
    vim.api.nvim_buf_set_extmark(state.buf, ns, row - 1, 0,
      { virt_text = chunks, virt_text_pos = "overlay" })
  else
    table.insert(chunks, 1, { indent_for(row, code) })
    vim.api.nvim_buf_set_extmark(state.buf, ns, row - 1, 0, { virt_lines = { chunks } })
  end
end

local function insert(code)
  local row, blank = target()
  if blank then
    vim.api.nvim_buf_set_lines(0, row - 1, row, false, { indent_for(row - 1, code) .. code })
  else
    vim.api.nvim_buf_set_lines(0, row, row, false, { indent_for(row, code) .. code })
    row = row + 1
  end
  vim.api.nvim_win_set_cursor(0, { row, #vim.api.nvim_get_current_line() })
  return row
end

-- Offer the other hypotheses for the line just inserted, if nothing moved.
local function offer(buf, row, tick, inserted, alternatives)
  if #alternatives < 2 or vim.api.nvim_get_current_buf() ~= buf
      or vim.api.nvim_win_get_cursor(0)[1] ~= row
      or vim.api.nvim_buf_get_changedtick(buf) ~= tick then
    return
  end
  local items = { { word = inserted, menu = "inserted" } }
  for _, alt in ipairs(alternatives) do
    if alt.code ~= inserted then
      table.insert(items, { word = alt.code, menu = alt.heard })
    end
  end
  if #items < 2 then return end
  local line = vim.api.nvim_get_current_line()
  local col = #line:match("^%s*") + 1
  vim.cmd("startinsert!")
  vim.schedule(function() vim.fn.complete(col, items) end)
end

local function dictate(on_done, nbest, lang_, wav, scope)
  local cmd = { "python3", here .. "dictate.py", lang_ or state.lang, wav or state.wav, scope or state.scope }
  if nbest then table.insert(cmd, 3, "--nbest") end
  vim.system(cmd,
    { text = true, cwd = here }, vim.schedule_wrap(function(out)
      if out.code ~= 0 then return on_done(nil, out.stderr) end
      on_done(vim.json.decode(out.stdout))
    end))
end

local function partial()
  if not state.rec or state.busy then return end
  state.busy = true
  dictate(function(result)
    state.busy = false
    if state.rec and result then ghost(result.code, result.heard) end
  end)
end

local function stop()
  local rec, buf, wav, scope, l = state.rec, state.buf, state.wav, state.scope, state.lang
  state.rec = nil
  state.timer:stop()
  state.timer:close()
  rec:kill("sigint")
  rec:wait()
  ghost("", "transcribing")
  dictate(function(result, err)
    vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
    if not result then
      vim.notify("ears: " .. (err or "failed"), vim.log.levels.ERROR)
    elseif result.code == "" then
      vim.notify("ears: heard nothing", vim.log.levels.WARN)
    elseif vim.api.nvim_get_current_buf() == buf then
      local row = insert(result.code)
      local tick = vim.api.nvim_buf_get_changedtick(buf)
      dictate(function(nbest)
        os.remove(wav)
        os.remove(scope)
        if nbest then offer(buf, row, tick, result.code, nbest.alternatives) end
      end, true, l, wav, scope)
      return
    end
    os.remove(wav)
    os.remove(scope)
  end)
end

local function toggle()
  if state.rec then return stop() end
  state.buf, state.lang, state.scope = vim.api.nvim_get_current_buf(), lang(), scope_file()
  state.wav = vim.fn.tempname() .. ".wav"
  local cmd = { "pw-record", "--rate", "16000", "--channels", "1", "--format", "s16" }
  local dev = device()
  if dev then vim.list_extend(cmd, { "--target", dev }) end
  table.insert(cmd, state.wav)
  state.rec = vim.system(cmd)
  ghost("", "listening")
  state.timer = vim.uv.new_timer()
  state.timer:start(PARTIAL_MS, PARTIAL_MS, vim.schedule_wrap(partial))
end

vim.keymap.set({ "n", "i" }, "<M-v>", toggle, { desc = "ears: dictate code" })

return { toggle = toggle, insert = insert, offer = offer }
