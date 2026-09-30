# VHS Demo for ears - Speech Recognition TUI
# Run with: vhs demo.tape

Output demo.gif

Set FontSize 14
Set Width 1100
Set Height 800
Set Theme "Catppuccin Mocha"
Set TypingSpeed 80ms

# Isolate config into throwaway dirs BEFORE launching, so recording this demo
# never mutates the real ~/.config/ears — the toggle keys below (f/p/t/a) call
# save_config(), and the ghost colour keys (o/O) write Alacritty and Hover
# styles under $XDG_CONFIG_HOME and $HOME. Hidden from the recording.
Hide
Type "export XDG_CONFIG_HOME=$(mktemp -d) XDG_RUNTIME_DIR=$(mktemp -d) HOME=$(mktemp -d)"
Enter
Type "clear"
Enter
Show

# Launch ears TUI (opens on the Configuration panel)
Type "ears"
Enter
Sleep 2.5s

# Toggle the lowercase and punctuation filters
Type "f"
Sleep 1s
Type "p"
Sleep 1s

# Cycle the ghost text colour, then the colour of its frozen start
Type "o"
Sleep 1s
Type "o"
Sleep 1s
Type "O"
Sleep 1.5s

# Logs panel
Tab
Sleep 1.5s
Type "j"
Sleep 500ms
Type "k"
Sleep 1s

# Live Transcription panel: progressive typing and auto-correction
Tab
Sleep 1.5s
Type "t"
Sleep 1s
Type "a"
Sleep 1.5s

# Back to Configuration, then quit with the vim command
Tab
Sleep 1.5s
Type ":"
Sleep 500ms
Type "q"
Enter
Sleep 500ms
