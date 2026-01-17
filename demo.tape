# VHS Demo for ears - Speech Recognition TUI
# Run with: vhs demo.tape

Output demo.gif

Set FontSize 14
Set Width 1000
Set Height 600
Set Theme "Catppuccin Mocha"
Set TypingSpeed 80ms

# Launch ears TUI
Type "ears"
Enter
Sleep 2s

# Show Status panel (default view)
Sleep 1.5s

# Navigate to Configuration panel
Tab
Sleep 1.5s

# Toggle lowercase filter
Type "f"
Sleep 1s

# Toggle punctuation filter
Type "p"
Sleep 1s

# Navigate to Logs panel
Tab
Sleep 1.5s

# Scroll through logs
Type "j"
Sleep 500ms
Type "j"
Sleep 500ms
Type "k"
Sleep 1s

# Navigate to Live Transcription panel
Tab
Sleep 1.5s

# Toggle VAD mode
Type "v"
Sleep 1.5s

# Toggle progressive typing
Type "t"
Sleep 1s

# Toggle auto-correction
Type "a"
Sleep 1s

# Disable VAD mode
Type "v"
Sleep 1s

# Go back to Status panel
Tab
Sleep 1.5s

# Show vim command mode
Type ":"
Sleep 500ms
Type "q"
Enter
Sleep 500ms
