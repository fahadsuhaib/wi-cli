# Keyboard: Shift+Enter and multi-line input

## Which key for a new line?
- **Alt+Enter** works in every terminal, in agent tabs (Claude Code, Codex) and everywhere else. Use it if unsure.
- **Shift+Enter** works only where the terminal can report it: kitty, Ghostty, WezTerm, foot, Alacritty, and
  iTerm2 with the key mapping below. Menu `I` (Info) says which case you are in.
- **Ctrl+J** also works everywhere.
- GNOME Terminal and Konsole cannot send Shift+Enter: use Alt+Enter. `wi` shows this once on the status line
  when an agent tab opens (never again after you have used Alt+Enter).

Claude Code and Codex insert a newline on LF (Ctrl+J) and submit on CR (Enter). In the classic
terminal protocol Shift+Enter sends the same byte as Enter, so a program cannot tell them apart.
Terminals with "enhanced key reporting" (kitty keyboard protocol, xterm modifyOtherKeys) can
report Shift+Enter; `wi` asks for it on start and translates it per tab.

## What wi does
- Agent tabs (started as an agent, or titled `claude` / `codex`): Shift+Enter and Ctrl+Enter send LF.
- Other tabs: Shift+Enter sends Enter. Alt+Enter always sends ESC CR (a newline in Claude Code and Codex), whatever the terminal's key protocol.
- Alt+Left / Alt+Right (and Ctrl+Left / Ctrl+Right) in a shell or agent tab send ESC b / ESC f, so they move by word in zsh, bash, fish, Claude Code and Codex; wi's own text fields take them (and Alt+Backspace) too.
- Every other enhanced report (Ctrl+letter, Esc, Ctrl+Space ...) is turned back into the classic bytes, so
  programs and the `Ctrl-Space` menu behave exactly as before. Pasted text is never touched.
- A program that turns the protocol on itself gets the raw reports instead.
- Menu `I` (Info) says `keys: enhanced keyboard active` once the terminal has sent such a report, or
  `keys: standard` when it has not, followed by the new-line key to use.

## Config (`~/.config/wicli/config.toml`)
    [defaults]
    key_protocol = "auto"   # or "off": do not ask the terminal, translate nothing
    shift_enter  = "auto"   # "newline" (always LF), "esc-cr" (always ESC CR), "enter" (always CR)

## Per terminal
- kitty, Ghostty, WezTerm, foot, Alacritty (recent): work natively, nothing to do.
- iTerm2: Settings > Profiles > Keys > Key Mappings, add Shift+Return, action "Send Hex Code", `0x0a`
  (or turn on "Report modifiers using CSI u" under Profiles > Terminal).
- macOS Terminal.app: Settings > Profiles > Keyboard, add a key: Shift+Return sending text `\012` (LF).
  It has no enhanced reporting, so this mapping applies in every tab, not only agent tabs.
- GNOME Terminal, Konsole, others without support: use Alt+Enter (or Ctrl+J) for a newline.
- tmux/ssh: wi sits in front of tmux, so nothing to set on the remote box.
