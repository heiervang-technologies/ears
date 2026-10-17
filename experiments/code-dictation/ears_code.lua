-- Push-to-talk code dictation for Neovim (experiment).
-- <leader>v (normal) or <M-v> (insert): start talking; press again to stop.
-- The utterance is transcribed with the buffer's names as context, translated
-- to Python or bash, and inserted as one line (one undo step).
local here = debug.getinfo(1, "S").source:sub(2):match("(.*/)")
local state = { rec = nil, wav = nil }

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

local function insert(code)
  local row = vim.api.nvim_win_get_cursor(0)[1]
  local cur = vim.api.nvim_get_current_line()
  if cur:match("^%s*$") then
    local indent = indent_for(row - 1, code)
    vim.api.nvim_buf_set_lines(0, row - 1, row, false, { indent .. code })
  else
    vim.api.nvim_buf_set_lines(0, row, row, false, { indent_for(row, code) .. code })
    row = row + 1
  end
  vim.api.nvim_win_set_cursor(0, { row, #vim.api.nvim_get_current_line() })
end

local function stop()
  local rec, wav = state.rec, state.wav
  state.rec = nil
  rec:kill("sigint")
  rec:wait()
  local buf, l, scope = vim.api.nvim_get_current_buf(), lang(), scope_file()
  vim.notify("ears: transcribing…")
  vim.system({ "python3", here .. "dictate.py", l, wav, scope }, { text = true, cwd = here },
    vim.schedule_wrap(function(out)
      os.remove(wav)
      os.remove(scope)
      if out.code ~= 0 then
        vim.notify("ears: " .. (out.stderr or "failed"), vim.log.levels.ERROR)
        return
      end
      local result = vim.json.decode(out.stdout)
      if result.code == "" then
        vim.notify("ears: heard nothing", vim.log.levels.WARN)
      elseif vim.api.nvim_get_current_buf() == buf then
        insert(result.code)
        vim.notify(("ears: %q → %s"):format(result.heard, result.code))
      end
    end))
end

local function toggle()
  if state.rec then return stop() end
  state.wav = vim.fn.tempname() .. ".wav"
  local cmd = { "pw-record", "--rate", "16000", "--channels", "1", "--format", "s16" }
  local dev = device()
  if dev then vim.list_extend(cmd, { "--target", dev }) end
  table.insert(cmd, state.wav)
  state.rec = vim.system(cmd)
  vim.notify("ears: listening (" .. lang() .. ")…")
end

vim.keymap.set("n", "<leader>v", toggle, { desc = "ears: dictate code" })
vim.keymap.set("i", "<M-v>", toggle, { desc = "ears: dictate code" })

return { toggle = toggle, insert = insert }
