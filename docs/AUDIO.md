# Voice dictation (`wi-audio`)

Talk to your agent instead of typing. Turn dictation on once; wi keeps listening, and each phrase you say is typed into
the focused pane, without pressing Enter, so you can edit it first.

**Audio stays on this machine.** The microphone is read by `wi-audio`, a separate small program; the speech model runs
on your own CPU. Nothing is uploaded, there is no telemetry, no cloud engine, and audio is never written to disk
(it lives in memory while a phrase is being heard). The only network use is the model download you confirm, from
huggingface.co. It works the same in a remote (ssh) tab: the text goes down the same pty as typing, and nothing is
installed on the remote box.

## Set up (once)

1. Get `wi-audio` from the [releases page](https://github.com/fahadsuhaib/wi-cli/releases):
   `wi-audio-macos-arm64.tar.gz`, `wi-audio-linux-x86_64.tar.gz` or `wi-audio-linux-aarch64.tar.gz`. Check it against
   `SHA256SUMS`, unpack it and put `wi-audio` on your `PATH` (for example `/usr/local/bin`).
   Or build it: `cd audio && cargo build --release` (needs cmake and a C++ compiler; on Linux `libasound2-dev`).
2. Download the speech model (asks first, shows the file, size and host, checks SHA256):

       wi-audio models install            # Moonshine Streaming small, 198.5 MB, plus Silero VAD, 1.2 MB
       wi-audio models install --tiny     # a 50 MB model for very small machines (less accurate)

3. Check it: `wi-audio doctor` lists the models and microphones, does a 3 second test recording and prints what it
   heard. It ends with one line for each thing to fix.
4. On a Mac the first use asks to let your **terminal app** (Terminal, iTerm, Ghostty...) use the microphone. If you
   said no: System Settings > Privacy & Security > Microphone, switch your terminal on, restart it.

## Using it in wi

Ctrl-Space, Space, type `dictate`, Enter (menu entry "Dictation (voice) on/off"). The right end of the status bar shows
the state in red:

| Indicator | Meaning |
| --- | --- |
| `● loading` | the engine is starting (the model loads in well under a second) |
| `● listening ▁▂▅` | waiting for speech; the bars are the microphone level |
| `● hearing ▅▇▅` | speech detected |
| `● ...` | a finished phrase is being converted to text |
| `held: "..."` | a phrase is waiting (see below) |

- Each phrase is typed into the **focused** pane; switch tab or pane and the next phrase goes there. Several phrases
  build one line. A phrase starts with a space when the character before the cursor is not blank.
- The keyboard is never blocked. If a phrase finishes within 0.3 s of a key press, it waits until you pause, so words
  never interleave with what you type.
- If the focused pane cannot take text (a menu, popup or preview is in front), the phrase is held, shown as
  `held: "..."`, typed when the pane is ready again, and dropped after `held_timeout_s` (30 s).
- Turn it off with the same menu entry, or by clicking the red indicator. It also turns itself off after
  `listen_idle_minutes` (10) without speech ("dictation off: 10 min of silence"), when wi quits, and when wi loses its
  terminal. Off means the microphone is released and the model is unloaded: no process is left running.
- Phrases are never sent to the agent for you. Enter stays with you.
- Short stray sounds are ignored, and a result that is only noise ("Thank you.", "you", "um") is dropped.

Memory: while listening, `wi-audio` holds the model, about 400 MB with the default model (about 140 MB with `--tiny`);
when dictation is off it is zero. Speed on an Apple M4 Pro: about 0.1 s from the end of a 3 second phrase to the text.

## Configuration (`~/.config/wicli/config.toml`)

    [transcribe]
    command = "wi-audio"        # the engine program; words split on spaces, quotes keep them together
    end_silence_ms = 700        # silence that ends a phrase
    min_speech_ms = 250         # speech that starts one
    vad_threshold = 0.5         # 0 to 1: how sure the voice detector must be
    listen_idle_minutes = 10    # auto-off after this long without speech (0 = never)
    input_device = ""           # part of a microphone's name; empty = the system default
    paste_leading_space = true
    held_timeout_s = 30
    # max_seconds = 120, silence_stop_ms = 0: for "dictate once", reserved

Dictation never starts by itself; there is no "on at start" setting.

## `wi-audio` on its own

    wi-audio doctor [--verify]          what is installed, microphones, a test recording (--verify checks the SHA256s)
    wi-audio dictate                    record once (Enter, or --silence-stop-ms 1500, or --max-seconds N stops), print the text
    wi-audio record --out note.wav      record once into a file (default: a private temp file, path printed)
    wi-audio transcribe note.wav        print the text of a wav file (any rate, long files are cut at quiet points)
    wi-audio listen                     what wi runs: one JSON line per event on stdout
    wi-audio models list | install | remove

The `listen` events (the contract wi reads; any program that prints these lines can be the engine):

    {"ev":"state","s":"loading"|"listening"|"hearing"|"transcribing"}
    {"ev":"level","v":0.4}
    {"ev":"final","text":"open the explorer tab","ms":71}
    {"ev":"error","kind":"no_model"|"no_mic"|"mic_denied"|"mic_silent"|"mic_stalled"|"engine","msg":"..."}
    {"ev":"off","why":"idle"|"end of audio"|"stopped"}

wi closes the engine's stdin to stop it; it also stops on SIGTERM and SIGINT.

Models live in `~/Library/Application Support/wicli/models` (macOS) or `~/.local/share/wicli/models` (Linux);
`wi-audio models remove` deletes them. `WICLI_AUDIO_MODELS=<dir>` uses another folder.

## Troubleshooting

| You see | Do |
| --- | --- |
| `no wi-audio found: download it ...` | install `wi-audio` as above and make sure it is on the `PATH` wi starts with |
| `no speech model yet` | `wi-audio models install` |
| `microphone blocked` / `no sound from the mic` | System Settings > Privacy & Security > Microphone: allow your terminal app, then restart the terminal |
| `the microphone stopped delivering audio` | the device was unplugged or taken by another app (Bluetooth headsets switching profile do this): turn dictation off and on |
| words are wrong in code terms | the model does not know identifiers (`Cargo.toml` can come out as "Cargo Tamil"); speak naturally, edit the line before sending. A word list for your project is planned |
| phrases cut in the middle of a thought | raise `end_silence_ms` (900-1200) |
| background noise starts phrases | raise `vad_threshold` (0.6-0.7) |

## Limits of this first version

English only. Linux microphone capture is built but untested. No live partial text, no voice commands ("send it"),
no echo guard (speakers playing sound into the microphone), no handling of a microphone that changes in the middle of
a session, no file transcription in wi (the `transcribe` command works on its own), no install of `wi-audio` from
inside wi. Do not use it while sound from your speakers reaches the microphone: the speakers' words get typed.

## Credits

Speech model: Moonshine Streaming by Useful Sensors (MIT), GGUF conversion by handy-computer; voice detection:
Silero VAD (MIT); runtime: transcribe.cpp (MIT, built on ggml).
