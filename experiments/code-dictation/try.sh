#!/usr/bin/env bash
# Open Neovim with code dictation loaded: try.sh file.py
exec nvim -c "luafile $(dirname "$(readlink -f "$0")")/ears_code.lua" "$@"
