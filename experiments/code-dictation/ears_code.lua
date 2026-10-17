-- Code dictation for Neovim (experiment).
--
-- Push-to-talk, <M-v> (Alt-v, normal or insert): start talking; press again
-- to stop. While you talk, the recording so far is re-transcribed every
-- PARTIAL_MS and the translated code shows as ghost text where it will land.
-- On stop the final transcription is inserted (one undo step). Beam-search
-- hypotheses follow in the background and, when they translate differently,
-- open as a completion menu over the inserted line.
--
-- With the ears VAD running: while this Neovim has focus (on any buffer but a
-- terminal or picker), it holds the ears input claim. ears then types nothing and
-- sends the words here instead: partials become ghost text, each utterance is
-- translated and inserted, and spoken ears commands act on the buffer
-- ("enter"/"new line" open a line, "scratch that" undoes, "back N" drops N
-- words, "backtrack" clears the line). Focus anything else and the claim
-- drops, so ears types normally there. A crash closes the socket, which
-- drops the claim too.
--
-- Neovim's mode decides what an utterance is. In insert mode it is dictation,
-- or an insert command said on its own ("clear line", "backspace three
-- times"). In any other mode it is a Vim command from a closed list ("dee
-- dee", "why why", "down five", "triple paste"), and anything else is
-- ignored, never inserted.
--
-- Inside dictation, "new line" breaks the line and a leading "indent" or
-- "dedent" shifts it a level; "double new line" or "dedent twice" repeat.
-- All phrases are configurable in ~/.config/ears/code.toml.
local here = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":p:h") .. "/"
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

-- Indentation for a line after `row`: the same as that line, one level more
-- after a Python block opener, one less for else/elif/except/finally, then
-- `shift` levels from a spoken indent or dedent.
local function indent_for(row, code, shift)
  local prev = vim.api.nvim_buf_get_lines(0, row - 1, row, false)[1] or ""
  local sw = vim.fn.shiftwidth()
  local width = #prev:match("^%s*")
  if prev:match(":%s*$") and vim.bo.filetype == "python" then
    width = width + sw
  end
  if code:match("^el") or code:match("^except") or code:match("^finally") then
    width = width - sw
  end
  return string.rep(" ", math.max(0, width + (shift or 0) * sw))
end

-- Where a line goes: over a blank current line, otherwise below it. A blank
-- line that already carries indentation (left by "new line" or "dedent")
-- keeps it.
local function target()
  local row = vim.api.nvim_win_get_cursor(0)[1]
  local line = vim.api.nvim_get_current_line()
  local blank = line:match("^%s*$") ~= nil
  return row, blank, blank and line ~= "" and line or nil
end

local function indent_at(row, blank, kept, code, shift)
  if kept then
    return string.rep(" ", math.max(0, #kept + (shift or 0) * vim.fn.shiftwidth()))
  end
  return indent_for(blank and row - 1 or row, code, shift)
end

local function ghost(code, heard, buf)
  buf = buf or state.buf
  vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  local row, blank, kept = target()
  local chunks = { { indent_at(row, blank, kept, code) }, { code ~= "" and code or "…", "EarsGhost" } }
  if heard and heard ~= "" then
    table.insert(chunks, { "   ← " .. heard, "EarsHeard" })
  end
  if blank then
    vim.api.nvim_buf_set_extmark(buf, ns, row - 1, 0, { virt_text = chunks, virt_text_pos = "overlay" })
  else
    vim.api.nvim_buf_set_extmark(buf, ns, row - 1, 0, { virt_lines = { chunks } })
  end
end

local function insert(code, shift)
  local row, blank, kept = target()
  local text = indent_at(row, blank, kept, code, shift) .. code
  if blank then
    vim.api.nvim_buf_set_lines(0, row - 1, row, false, { text })
  else
    vim.api.nvim_buf_set_lines(0, row, row, false, { text })
    row = row + 1
  end
  vim.api.nvim_win_set_cursor(0, { row, #text })
  return row
end

-- Insert an utterance's lines, one under the other, as one undo step.
local function insert_lines(lines)
  local row
  for i, line in ipairs(lines) do
    if i == 1 then
      row = insert(line.code, line.shift)
    else
      pcall(vim.cmd, "undojoin")
      local text = indent_for(row, line.code, line.shift) .. line.code
      vim.api.nvim_buf_set_lines(0, row, row, false, { text })
      row = row + 1
      vim.api.nvim_win_set_cursor(0, { row, #text })
    end
  end
  return row
end

local function dictate(on_done, nbest, lang_, wav, scope)
  local cmd = { "python3", here .. "dictate.py", lang_ or state.lang, wav or state.wav, scope or state.scope }
  if nbest then table.insert(cmd, 3, "--nbest") end
  vim.system(cmd, { text = true, cwd = here }, vim.schedule_wrap(function(out)
    if out.code ~= 0 then return on_done(nil, out.stderr) end
    on_done(vim.json.decode(out.stdout))
  end))
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
  local col = #vim.api.nvim_get_current_line():match("^%s*") + 1
  vim.cmd("startinsert!")
  vim.schedule(function() vim.fn.complete(col, items) end)
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
    elseif #result.lines == 0 then
      vim.notify("ears: heard nothing", vim.log.levels.WARN)
    elseif vim.api.nvim_get_current_buf() == buf then
      local row = insert_lines(result.lines)
      if #result.lines == 1 and result.lines[1].code == result.code then
        local tick = vim.api.nvim_buf_get_changedtick(buf)
        dictate(function(nbest)
          os.remove(wav)
          os.remove(scope)
          if nbest then offer(buf, row, tick, result.code, nbest.alternatives) end
        end, true, l, wav, scope)
        return
      end
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

-- VAD path: the ears input claim ---------------------------------------------

local runtime = os.getenv("XDG_RUNTIME_DIR") or "/tmp"
-- Terminals and pickers keep ordinary ears typing.
local SKIP_BUFTYPES = { terminal = true, prompt = true }
-- Whether the window holding this Neovim has focus right now; after this,
-- FocusGained/FocusLost keep it current. Under tmux, assume yes.
local function has_focus()
  if os.getenv("TMUX") then return true end
  local ok, active = pcall(vim.json.decode, vim.fn.system({ "hyprctl", "activewindow", "-j" }))
  if not ok or type(active) ~= "table" or not active.pid then return true end
  local pid = vim.fn.getpid()
  while pid and pid > 1 do
    if pid == active.pid then return true end
    local stat = io.open("/proc/" .. pid .. "/stat")
    if not stat then break end
    pid = tonumber(stat:read("*a"):match("^%d+ %b() %S+ (%d+)"))
    stat:close()
  end
  return false
end

local vad = { events = nil, claim = nil, focused = has_focus(), translator = nil, pending = {}, next_id = 0 }

-- Loading again (`:luafile`) replaces the previous instance instead of
-- leaving it connected, which would insert every utterance twice.
if _G.ears_code_vad then
  local old = _G.ears_code_vad
  old.dead = true
  for _, pipe in ipairs({ old.events, old.claim }) do
    if pipe and not pipe:is_closing() then pipe:close() end
  end
  if old.translator then old.translator:kill("sigterm") end
end
_G.ears_code_vad = vad

local function scope_names()
  local path = scope_file()
  local names = vim.split(vim.fn.readfile(path)[1] or "", " ", { trimempty = true })
  os.remove(path)
  return names
end

-- One translator process for all utterances: no startup cost per partial.
local function translate(text, mode, on_reply)
  if not vad.translator then
    local buffered = ""
    vad.translator = vim.system({ "python3", here .. "dictate.py", "--serve" }, {
      cwd = here,
      stdin = true,
      stdout = function(_, data)
        if not data then return end
        buffered = buffered .. data
        for line in buffered:gmatch("([^\n]*)\n") do
          local ok, reply = pcall(vim.json.decode, line)
          local done = ok and vad.pending[reply.id]
          if done then
            vad.pending[reply.id] = nil
            vim.schedule(function() done(reply) end)
          end
        end
        buffered = buffered:match("[^\n]*$")
      end,
    }, function() vad.translator = nil end)
  end
  vad.next_id = vad.next_id + 1
  vad.pending[vad.next_id] = on_reply
  vad.translator:write(vim.json.encode({
    id = vad.next_id, lang = lang(), text = text, mode = mode,
    scope = mode == "insert" and scope_names() or {},
  }) .. "\n")
end

local function voice_command(name)
  local line = vim.api.nvim_get_current_line()
  if name == "enter" or name == "new_line" then
    insert_lines({ { code = "", shift = 0 } })
  elseif name == "undo" then
    pcall(vim.cmd, "undo")
  elseif name == "backtrack" then
    vim.api.nvim_set_current_line(line:match("^%s*"))
  elseif name:match("^back:") then
    for _ = 1, tonumber(name:match("%d+")) do
      line = line:gsub("%s*%S+%s*$", "")
    end
    vim.api.nvim_set_current_line(line)
  end
end

local function feed(keys, heard)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(keys, true, false, true), "n", false)
  vim.api.nvim_echo({ { "ears: " .. heard .. " → " .. keys, "EarsHeard" } }, false, {})
end

-- Prose goes in where the cursor is, like typing would.
local function put_text(text)
  local col = vim.api.nvim_win_get_cursor(0)[2]
  local before = vim.api.nvim_get_current_line():sub(col, col)
  if before ~= "" and not before:match("%s") then text = " " .. text end
  vim.api.nvim_put({ text }, "c", false, true)
end

-- Neovim's mode decides what an utterance is: in insert mode it is dictation
-- (or an insert command like "clear line"), anywhere else a Vim command from
-- the closed list in code.toml, or nothing at all.
local function voice_mode()
  return vim.api.nvim_get_mode().mode:match("^[iR]") and "insert" or "normal"
end

local function on_event(ev)
  if vad.dead or not vad.claim or state.rec then return end
  local buf = vim.api.nvim_get_current_buf()
  local mode = voice_mode()
  if ev == "SpeechStarted" then
    ghost("", "listening", buf)
  elseif type(ev) ~= "table" then
    return
  elseif ev.Partial then
    local heard = ev.Partial.text
    if mode ~= "insert" or lang() == "text" then return ghost("", heard, buf) end
    translate(heard, mode, function(reply)
      if vad.claim and reply.lines and reply.lines[1] then ghost(reply.lines[1].code, heard, buf) end
    end)
  elseif ev.SegmentCompleted then
    local heard = ev.SegmentCompleted.text
    ghost("", heard, buf)
    translate(heard, mode, function(reply)
      vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
      if vim.g.ears_debug then io.stderr:write(mode .. " " .. vim.inspect(reply) .. "\n") end
      if vim.api.nvim_get_current_buf() ~= buf then return end
      if reply.keys and reply.keys ~= vim.NIL then
        feed(reply.keys, heard)
      elseif mode ~= "insert" then
        vim.api.nvim_echo({ { "ears: not a command: " .. heard, "WarningMsg" } }, false, {})
      elseif lang() == "text" then
        put_text(heard)
      elseif reply.lines and #reply.lines > 0 then
        insert_lines(reply.lines)
      end
    end)
  elseif ev.VoiceCommand then
    vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
    voice_command(ev.VoiceCommand.name)
  elseif ev.SegmentDiscarded then
    vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  end
end

-- Newline-delimited JSON from a socket, until it closes.
local function connect(path, on_line, on_close)
  local pipe = vim.uv.new_pipe(false)
  local buffered = ""
  pipe:connect(path, function(err)
    if err then
      pipe:close()
      return vim.schedule(function() on_close(pipe) end)
    end
    pipe:read_start(function(_, data)
      if not data then
        if not pipe:is_closing() then pipe:close() end
        return vim.schedule(function() on_close(pipe) end)
      end
      buffered = buffered .. data
      for line in buffered:gmatch("([^\n]*)\n") do
        vim.schedule(function() on_line(line) end)
      end
      buffered = buffered:match("[^\n]*$")
    end)
  end)
  return pipe
end

local resolve

-- Keep listening to ears events; retry while the VAD is not running.
local function listen()
  vad.events = connect(runtime .. "/ears.sock", function(line)
    local ok, ev = pcall(vim.json.decode, line)
    if ok then on_event(ev) end
  end, function(pipe)
    if vad.events ~= pipe then return end
    vad.events = nil
    if vad.claim then
      if not vad.claim:is_closing() then vad.claim:close() end
      vad.claim = nil
    end
    if not vad.dead then vim.defer_fn(function() listen(); resolve() end, 3000) end
  end)
end

-- Hold the claim exactly while this Neovim is focused on a code buffer.
resolve = function()
  local want = vad.focused and vad.events ~= nil and not SKIP_BUFTYPES[vim.bo.buftype]
    and vim.api.nvim_get_mode().mode ~= "t"
  if want and not vad.claim then
    local pipe = connect(runtime .. "/ears-cmd.sock", function() end, function(pipe)
      if vad.claim == pipe then vad.claim = nil end
    end)
    pipe:write("claim\n")
    vad.claim = pipe
  elseif not want and vad.claim then
    local pipe = vad.claim
    vad.claim = nil
    if not pipe:is_closing() then pipe:close() end -- closing releases it
    vim.api.nvim_buf_clear_namespace(0, ns, 0, -1)
  end
end

local group = vim.api.nvim_create_augroup("ears_code", { clear = true })
vim.api.nvim_create_autocmd("FocusGained", { group = group, callback = function() vad.focused = true; resolve() end })
vim.api.nvim_create_autocmd("FocusLost", { group = group, callback = function() vad.focused = false; resolve() end })
vim.api.nvim_create_autocmd({ "BufEnter", "FileType", "ModeChanged" }, { group = group, callback = function() resolve() end })
listen()
vim.defer_fn(resolve, 300)

vim.keymap.set({ "n", "i" }, "<M-v>", toggle, { desc = "ears: dictate code" })

return { toggle = toggle, insert = insert, insert_lines = insert_lines, offer = offer,
  on_event = on_event, state = vad }
