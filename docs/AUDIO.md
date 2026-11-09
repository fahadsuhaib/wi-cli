# Voice dictation

macOS on Apple Silicon only for now (Linux later). Talk to your agent: each phrase you say is typed into the focused
pane, without Enter, so you can edit it first.

## Use it

Click the `○ mic` at the right end of the top bar, or choose **Dictation** in the menu (Ctrl-Space, Space, `dictate`).
The first time, wi asks (one short question per step) to install a small voice helper (about 2 MB, from this
repository's releases) and to download the speech model (296 MB, from huggingface.co), then tests your microphone for
3 seconds. Each answer can be "not now"; it is remembered and not asked again until you choose Dictation again.
macOS asks once to let your terminal app use the microphone; if you said no, allow it in System Settings > Privacy &
Security > Microphone.

The mic shows `● listening`, `● hearing` and `● ...` while on. Click it again to stop. Choosing Dictation while it is on
shows a small screen: Enter turns it off, **O** opens the options: **A** press Enter by itself after 3 seconds of quiet (off until you turn it on; typing or switching tabs cancels it), **T** test the microphone, **D** download the speech model again (fixes a damaged file or updates it), **R** remove voice (deletes
the model and the helper, frees 300 MB).

## Privacy

Audio is read and converted on this computer only. Nothing is uploaded, there is no telemetry, and audio is never
written to disk. The only downloads are the two you confirm (SHA256-checked). The microphone is open only while the
mic shows `●`; it turns itself off after 10 minutes without speech, when wi quits, and when wi loses its terminal.

## Config

    [transcribe]
    enabled = true      # false hides the mic button and disables the entry
    idle_minutes = 10   # 0 = never switch off by itself

## Remove it

Dictation > **O** > **R**. Or delete the helper (next to the `wi` binary, or `~/.local/share/wicli/bin/wi-audio`) and the
model folder (`~/Library/Application Support/wicli/models`).

## Limits

English only, no cloud engine, no voice commands yet. Technical words are not always right (the model does not know
identifiers): check the line before pressing Enter.
